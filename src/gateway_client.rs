//! Gateway-WebSocket-Client (0.2.0: read-only Streaming-Prototyp).
//!
//! Verbindet sich als OpenClaw-Operator mit dem Gateway, meldet sich über den
//! in `device_identity.rs` implementierten Ed25519-Signaturvertrag an und
//! abonniert die Transkript-/Nachrichtenereignisse des konfigurierten
//! Zielkanals. Bewusst rein lesend: `chat.send` (aktives Auslösen einer
//! Antwort) kommt erst in einer späteren Version - hier wird nur
//! protokolliert, was das Gateway an Events liefert.
//!
//! Frame-Formen und Methodennamen sind gegen den tatsächlichen
//! OpenClaw-Quellcode geprüft (nicht nur gegen die Doku):
//!   - `packages/gateway-protocol/src/schema/frames.ts` (Frame-Formen)
//!   - `src/gateway/server-methods/sessions-subscriptions.ts`
//!     (`sessions.messages.subscribe` erwartet `params.key`, nicht
//!     `sessionKey` o. Ä.)
//!   - `packages/gateway-protocol/src/client-info.ts` (`GATEWAY_CLIENT_IDS`/
//!     `GATEWAY_CLIENT_MODES`): `client.id`/`client.mode` sind geschlossene
//!     Enums, keine freien Strings - ein selbst erfundener Wert wie das
//!     ursprüngliche `"openclaw-voicebridge"`/`"operator"` wird vom Gateway
//!     mit `INVALID_REQUEST` abgelehnt, bevor überhaupt die Geräte-Signatur
//!     geprüft wird. `"operator"` ist nur für das separate `role`-Feld
//!     gültig (eigenes, unabhängiges Enum `{"operator","node"}`) - nicht für
//!     `client.mode`. `openclaw-probe`/`probe` ist der für einen
//!     Diagnose-Client wie `--probe-gateway` vorgesehene Eintrag.

use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::{timeout, Duration};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::config::{Config, OpenClawConfig};
use crate::device_identity::{build_device_auth_payload_v3, DeviceIdentity, V3PayloadParams};

/// Scopes, die für den read-only Prototyp UND das später geplante
/// `chat.send` ausreichen (siehe `docs/gateway/clients`, Abschnitt "Bereiche
/// auswählen") - einmal angefordert, damit eine spätere volle Integration
/// keinen erneuten Kopplungs-Vorgang mit anderen Scopes auslöst.
const REQUESTED_SCOPES: &[&str] = &["operator.read", "operator.write"];
/// Muss ein Wert aus `GATEWAY_CLIENT_IDS` in
/// `packages/gateway-protocol/src/client-info.ts` sein - geschlossenes Enum,
/// keine freie Kennung. `openclaw-probe` ist der dort für Diagnose-Clients
/// vorgesehene Eintrag.
const CLIENT_ID: &str = "openclaw-probe";
/// Muss ein Wert aus `GATEWAY_CLIENT_MODES` in derselben Datei sein - ein
/// eigenes, geschlossenes Enum, NICHT dasselbe wie `role` weiter unten.
const CLIENT_MODE: &str = "probe";
/// Muss `"operator"` oder `"node"` sein (`GATEWAY_ROLES` in
/// `src/gateway/role-policy.ts`) - unabhängig vom `client.mode`-Enum oben.
const ROLE: &str = "operator";

/// Events, über die der Prototyp laut Roadmap berichten soll. Alles andere
/// (`tick`, `heartbeat`, `presence`, ...) landet nur auf `debug`-Ebene, damit
/// das Log nicht von reinem Transport-Keepalive überflutet wird.
fn describe_event(event: &str) -> Option<&'static str> {
    match event {
        "chat" => Some("Chat-Update (deltaText/final)"),
        "session.message" => Some("Sitzungs-Nachricht"),
        "session.operation" => Some("Sitzungs-Operation"),
        "session.tool" => Some("Tool-Ereignis"),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct InboundFrame {
    #[serde(rename = "type")]
    frame_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    payload: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<GatewayErrorPayload>,
    #[serde(default)]
    event: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GatewayErrorPayload {
    code: String,
    message: String,
    #[serde(default)]
    details: Option<serde_json::Value>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Baut den signierten `connect`-Request. Reine Funktion (abgesehen vom
/// Signieren selbst) - unabhängig von einer echten Verbindung testbar.
fn build_connect_request(
    cfg: &OpenClawConfig,
    identity: &DeviceIdentity,
    nonce: &str,
    signed_at_ms: i64,
    request_id: &str,
) -> serde_json::Value {
    let scopes: Vec<String> = REQUESTED_SCOPES.iter().map(|s| s.to_string()).collect();
    // `client.platform` fließt beim Gateway 1:1 in die Signaturprüfung ein
    // (`connectParams.client.platform`, siehe `handshake-auth-helpers.ts`) -
    // der hier gesendete Wert muss deshalb exakt der sein, mit dem auch
    // signiert wird.
    let platform = std::env::consts::OS;
    let token = if cfg.gateway_token.is_empty() {
        None
    } else {
        Some(cfg.gateway_token.as_str())
    };
    let device_id = identity.device_id();

    let payload = build_device_auth_payload_v3(&V3PayloadParams {
        device_id: &device_id,
        client_id: CLIENT_ID,
        client_mode: CLIENT_MODE,
        role: ROLE,
        scopes: &scopes,
        signed_at_ms,
        token,
        nonce,
        platform: Some(platform),
        // `client.deviceFamily` wird nicht mitgeschickt - optionales Feld,
        // die Gegenstelle normalisiert ein fehlendes Feld auf denselben
        // leeren String, mit dem hier signiert wird (siehe
        // `normalize_device_metadata_for_auth`).
        device_family: None,
    });
    let signature = identity.sign_payload(&payload);

    let mut auth = serde_json::Map::new();
    if let Some(token) = token {
        auth.insert("token".to_string(), json!(token));
    }

    json!({
        "type": "req",
        "id": request_id,
        "method": "connect",
        "params": {
            "minProtocol": 4,
            "maxProtocol": 4,
            "client": {
                "id": CLIENT_ID,
                "version": env!("CARGO_PKG_VERSION"),
                "platform": platform,
                "mode": CLIENT_MODE,
            },
            "role": ROLE,
            "scopes": scopes,
            "caps": [],
            "commands": [],
            "permissions": {},
            "auth": auth,
            "locale": "de-DE",
            "userAgent": format!("openclaw-voicebridge/{}", env!("CARGO_PKG_VERSION")),
            "device": {
                "id": device_id,
                "publicKey": identity.public_key_b64url(),
                "signature": signature,
                "signedAt": signed_at_ms,
                "nonce": nonce,
            }
        }
    })
}

fn build_subscribe_request(target_channel: &str, request_id: &str) -> serde_json::Value {
    json!({
        "type": "req",
        "id": request_id,
        "method": "sessions.messages.subscribe",
        "params": {
            "key": target_channel,
        }
    })
}

fn pairing_required_message(error: &GatewayErrorPayload) -> Option<String> {
    let code = error
        .details
        .as_ref()
        .and_then(|d| d.get("code"))
        .and_then(|c| c.as_str());
    if code != Some("PAIRING_REQUIRED") {
        return None;
    }
    let request_id = error
        .details
        .as_ref()
        .and_then(|d| d.get("requestId"))
        .and_then(|v| v.as_str());
    Some(format!(
        "Das Gateway verlangt eine einmalige Geräte-Kopplung für diese Bridge, bevor sie \
         Scopes bekommt. Auf dem Gateway-Host ausführen: `openclaw devices list`{}, dann \
         `openclaw devices approve <requestId>` - anschließend die Bridge neu starten.",
        request_id
            .map(|id| format!(" (requestId: {id})"))
            .unwrap_or_default()
    ))
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Liest genau einen Frame vom Socket, mit Timeout - eine Gegenstelle, die
/// gar nicht antwortet (falscher Port, kein OpenClaw-Gateway), soll den
/// Prototyp nicht unbegrenzt blockieren.
async fn read_frame(ws: &mut WsStream, timeout_secs: u64) -> Result<InboundFrame> {
    let msg = timeout(Duration::from_secs(timeout_secs), ws.next())
        .await
        .context("Timeout beim Warten auf eine Gateway-Antwort")?
        .context("Gateway-Verbindung wurde beendet, bevor eine Antwort kam")??;
    let text = match msg {
        Message::Text(text) => text,
        Message::Close(frame) => {
            bail!("Gateway hat die Verbindung geschlossen: {frame:?}");
        }
        other => bail!("Unerwarteter Frame-Typ vom Gateway: {other:?}"),
    };
    serde_json::from_str(&text).with_context(|| format!("Kann Gateway-Frame nicht lesen: {text}"))
}

/// Wartet auf die `res`-Antwort mit `id == expected_id` und überspringt dabei
/// Events, die zwischenzeitlich ankommen können (z. B. `tick`/`presence`) -
/// das Protokoll garantiert keine strikte Antwort-vor-nächstem-Event-Ordnung.
async fn read_response(
    ws: &mut WsStream,
    timeout_secs: u64,
    expected_id: &str,
) -> Result<InboundFrame> {
    loop {
        let frame = read_frame(ws, timeout_secs).await?;
        if frame.frame_type == "res" {
            if frame.id.as_deref() != Some(expected_id) {
                bail!(
                    "Antwort-ID passt nicht zur Anfrage (erwartet {expected_id}, bekommen {:?})",
                    frame.id
                );
            }
            return Ok(frame);
        }
        debug!(frame_type = %frame.frame_type, event = ?frame.event, "Frame vor der erwarteten Antwort übersprungen");
    }
}

/// Verbindet sich einmalig, meldet sich an, abonniert den konfigurierten
/// Zielkanal und protokolliert Events, bis der Prozess beendet wird (Strg+C)
/// oder die Verbindung abbricht.
pub async fn run_read_only_probe(cfg: &Config) -> Result<()> {
    let identity_path = crate::device_identity::identity_path()?;
    let identity = DeviceIdentity::load_or_create(&identity_path)
        .context("Kann Geräteidentität nicht laden/anlegen")?;
    info!(
        device_id = %identity.device_id(),
        path = %identity_path.display(),
        "Geräteidentität geladen"
    );

    let url = format!(
        "ws://{}:{}",
        cfg.openclaw.gateway_host, cfg.openclaw.gateway_port
    );
    info!(%url, "Verbinde mit OpenClaw-Gateway");
    let (mut ws, _response) = tokio_tungstenite::connect_async(&url)
        .await
        .with_context(|| format!("Kann keine WebSocket-Verbindung zu {url} aufbauen"))?;

    let timeout_secs = cfg.openclaw.timeout_secs;

    // 1. Auf die Challenge warten - das Gateway lehnt jeden ersten Frame ab,
    // der keine `connect`-Anfrage ist, aber schickt uns davor die Nonce, die
    // wir signieren müssen.
    let challenge = read_frame(&mut ws, timeout_secs).await?;
    if challenge.frame_type != "event" || challenge.event.as_deref() != Some("connect.challenge") {
        bail!(
            "Erster Frame vom Gateway war keine connect.challenge (bekommen: {:?}/{:?}) - \
             läuft dort wirklich ein OpenClaw-Gateway?",
            challenge.frame_type,
            challenge.event
        );
    }
    let nonce = challenge
        .payload
        .as_ref()
        .and_then(|p| p.get("nonce"))
        .and_then(|n| n.as_str())
        .context("connect.challenge enthielt keine nonce")?
        .to_string();

    // 2. Signierten connect-Request senden.
    let connect_id = uuid::Uuid::new_v4().to_string();
    let connect_req =
        build_connect_request(&cfg.openclaw, &identity, &nonce, now_ms(), &connect_id);
    ws.send(Message::Text(connect_req.to_string()))
        .await
        .context("Kann connect-Anfrage nicht senden")?;

    let hello = read_response(&mut ws, timeout_secs, &connect_id).await?;
    if hello.ok != Some(true) {
        let error = hello
            .error
            .context("Gateway lehnte connect ab, ohne einen Fehler mitzuschicken")?;
        if let Some(guidance) = pairing_required_message(&error) {
            bail!(guidance);
        }
        bail!(
            "Gateway-Connect fehlgeschlagen ({}): {}",
            error.code,
            error.message
        );
    }
    let hello_payload = hello.payload.unwrap_or_default();
    info!(
        protocol = ?hello_payload.get("protocol"),
        scopes = ?hello_payload.get("auth").and_then(|a| a.get("scopes")),
        "Gateway-Verbindung hergestellt (hello-ok)"
    );

    // 3. Zielkanal abonnieren.
    let subscribe_id = uuid::Uuid::new_v4().to_string();
    let subscribe_req = build_subscribe_request(&cfg.openclaw.target_channel, &subscribe_id);
    ws.send(Message::Text(subscribe_req.to_string()))
        .await
        .context("Kann sessions.messages.subscribe nicht senden")?;
    let subscribe_res = read_response(&mut ws, timeout_secs, &subscribe_id).await?;
    if subscribe_res.ok != Some(true) {
        let error = subscribe_res
            .error
            .context("sessions.messages.subscribe schlug fehl, ohne einen Fehler mitzuschicken")?;
        bail!(
            "sessions.messages.subscribe fehlgeschlagen ({}): {}",
            error.code,
            error.message
        );
    }
    info!(
        target_channel = %cfg.openclaw.target_channel,
        "Zielkanal abonniert - protokolliere eingehende Events (Strg+C zum Beenden)"
    );

    // 4. Events protokollieren, bis Strg+C oder Verbindungsende.
    loop {
        tokio::select! {
            frame = ws.next() => {
                let Some(frame) = frame else {
                    info!("Gateway hat die Verbindung beendet");
                    return Ok(());
                };
                let frame = frame.context("Fehler beim Lesen vom Gateway")?;
                let Message::Text(text) = frame else { continue };
                let parsed: InboundFrame = match serde_json::from_str(&text) {
                    Ok(f) => f,
                    Err(e) => {
                        warn!(error = %e, %text, "Konnte Gateway-Frame nicht parsen");
                        continue;
                    }
                };
                if parsed.frame_type != "event" {
                    continue;
                }
                let Some(event) = parsed.event else { continue };
                match describe_event(&event) {
                    Some(description) => {
                        info!(%event, description, payload = ?parsed.payload, "Gateway-Event");
                    }
                    None => {
                        debug!(%event, payload = ?parsed.payload, "Gateway-Event (Transport/Keepalive)");
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Beende auf Anfrage (Strg+C)");
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_identity::DeviceIdentity;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    fn temp_identity() -> DeviceIdentity {
        let path = std::env::temp_dir().join(format!(
            "openclaw-voicebridge-test-gwclient-{}.json",
            uuid::Uuid::new_v4()
        ));
        DeviceIdentity::load_or_create(&path).expect("Identität sollte anlegbar sein")
    }

    #[test]
    fn connect_request_has_the_documented_frame_shape() {
        let cfg = OpenClawConfig {
            target_channel: "agent:main:voice-assistant".to_string(),
            gateway_token: crate::config::GatewayToken::default(),
            ..Default::default()
        };
        let identity = temp_identity();
        let req = build_connect_request(&cfg, &identity, "test-nonce", 1_700_000_000_000, "req-1");

        assert_eq!(req["type"], "req");
        assert_eq!(req["id"], "req-1");
        assert_eq!(req["method"], "connect");
        assert_eq!(req["params"]["minProtocol"], 4);
        assert_eq!(req["params"]["maxProtocol"], 4);
        assert_eq!(req["params"]["role"], "operator");
        assert_eq!(req["params"]["client"]["id"], "openclaw-probe");
        assert_eq!(req["params"]["client"]["mode"], "probe");
        assert_eq!(
            req["params"]["scopes"],
            json!(["operator.read", "operator.write"])
        );
        assert_eq!(req["params"]["device"]["nonce"], "test-nonce");
        assert_eq!(req["params"]["device"]["signedAt"], 1_700_000_000_000i64);
    }

    /// Regression: `client.id`/`client.mode` sind vom Gateway als geschlossene
    /// Enums validiert (`GATEWAY_CLIENT_IDS`/`GATEWAY_CLIENT_MODES` in
    /// `packages/gateway-protocol/src/client-info.ts`), keine freien Strings.
    /// Ein selbst erfundener Wert wie das ursprüngliche
    /// `"openclaw-voicebridge"`/`"operator"` wurde vom Gateway mit
    /// `INVALID_REQUEST` abgelehnt, noch bevor die Geräte-Signatur geprüft
    /// wurde - das fiel nur gegen ein echtes Gateway auf, nicht hier. Diese
    /// Liste ist der Stand der beiden Enums zum Zeitpunkt dieses Fixes;
    /// stimmt sie nicht mehr mit dem tatsächlichen OpenClaw-Quellcode
    /// überein, muss sie erneut abgeglichen werden.
    #[test]
    fn client_id_and_mode_are_in_the_gateways_closed_enums() {
        const KNOWN_GOOD_CLIENT_IDS: &[&str] = &[
            "webchat-ui",
            "openclaw-control-ui",
            "openclaw-browser-copilot",
            "openclaw-tui",
            "webchat",
            "cli",
            "gateway-client",
            "openclaw-macos",
            "openclaw-linux",
            "openclaw-ios",
            "openclaw-watchos",
            "openclaw-android",
            "node-host",
            "openclaw-worker",
            "test",
            "fingerprint",
            "openclaw-probe",
        ];
        const KNOWN_GOOD_CLIENT_MODES: &[&str] = &[
            "webchat", "cli", "ui", "backend", "node", "worker", "probe", "test",
        ];
        assert!(
            KNOWN_GOOD_CLIENT_IDS.contains(&CLIENT_ID),
            "CLIENT_ID {CLIENT_ID:?} ist kein bekannter Wert aus GATEWAY_CLIENT_IDS"
        );
        assert!(
            KNOWN_GOOD_CLIENT_MODES.contains(&CLIENT_MODE),
            "CLIENT_MODE {CLIENT_MODE:?} ist kein bekannter Wert aus GATEWAY_CLIENT_MODES"
        );
        // Die beiden Enums sind unabhängig - "operator" ist z. B. für `role`
        // gültig, aber für keines der beiden hier.
        assert!(!KNOWN_GOOD_CLIENT_MODES.contains(&"operator"));
    }

    #[test]
    fn connect_request_omits_the_token_field_when_no_token_is_configured() {
        let cfg = OpenClawConfig::default();
        let identity = temp_identity();
        let req = build_connect_request(&cfg, &identity, "n", 1, "id");
        assert!(req["params"]["auth"].get("token").is_none());
    }

    #[test]
    fn connect_request_includes_the_token_when_configured() {
        let cfg = OpenClawConfig {
            gateway_token: crate::config::GatewayToken::from("shared-secret".to_string()),
            ..Default::default()
        };
        let identity = temp_identity();
        let req = build_connect_request(&cfg, &identity, "n", 1, "id");
        assert_eq!(req["params"]["auth"]["token"], "shared-secret");
    }

    /// Die eigentliche Absicherung: Die Signatur im connect-Request muss mit
    /// dem übertragenen Public Key tatsächlich verifizierbar sein - das
    /// prüft, dass Payload-Aufbau und Signieren zueinander passen, ohne ein
    /// echtes Gateway zu brauchen.
    #[test]
    fn connect_request_signature_verifies_against_the_transmitted_public_key() {
        let cfg = OpenClawConfig {
            target_channel: "agent:main:voice-assistant".to_string(),
            ..Default::default()
        };
        let identity = temp_identity();
        let req = build_connect_request(&cfg, &identity, "the-nonce", 1_700_000_000_000, "id");

        let scopes: Vec<String> = REQUESTED_SCOPES.iter().map(|s| s.to_string()).collect();
        let expected_payload = build_device_auth_payload_v3(&V3PayloadParams {
            device_id: req["params"]["device"]["id"].as_str().unwrap(),
            client_id: CLIENT_ID,
            client_mode: CLIENT_MODE,
            role: ROLE,
            scopes: &scopes,
            signed_at_ms: 1_700_000_000_000,
            token: None,
            nonce: "the-nonce",
            platform: Some(std::env::consts::OS),
            device_family: None,
        });

        let public_key_raw = base64::Engine::decode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            req["params"]["device"]["publicKey"].as_str().unwrap(),
        )
        .unwrap();
        let verifying_key = VerifyingKey::from_bytes(&public_key_raw.try_into().unwrap()).unwrap();
        let signature_raw = base64::Engine::decode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            req["params"]["device"]["signature"].as_str().unwrap(),
        )
        .unwrap();
        let signature = Signature::from_bytes(&signature_raw.try_into().unwrap());
        assert!(verifying_key
            .verify(expected_payload.as_bytes(), &signature)
            .is_ok());
    }

    #[test]
    fn subscribe_request_uses_key_as_the_param_name() {
        let req = build_subscribe_request("agent:main:voice-assistant", "sub-1");
        assert_eq!(req["method"], "sessions.messages.subscribe");
        assert_eq!(req["params"]["key"], "agent:main:voice-assistant");
        assert!(req["params"].get("sessionKey").is_none());
    }

    #[test]
    fn pairing_required_error_produces_actionable_guidance() {
        let error = GatewayErrorPayload {
            code: "INVALID_REQUEST".to_string(),
            message: "device pairing required".to_string(),
            details: Some(json!({"code": "PAIRING_REQUIRED", "requestId": "req-42"})),
        };
        let message = pairing_required_message(&error).expect("sollte erkannt werden");
        assert!(message.contains("openclaw devices approve"), "{message}");
        assert!(message.contains("req-42"), "{message}");
    }

    #[test]
    fn non_pairing_errors_produce_no_guidance() {
        let error = GatewayErrorPayload {
            code: "UNAUTHORIZED".to_string(),
            message: "bad token".to_string(),
            details: None,
        };
        assert!(pairing_required_message(&error).is_none());
    }

    #[test]
    fn describe_event_flags_the_roadmap_events_and_nothing_else() {
        for event in [
            "chat",
            "session.message",
            "session.operation",
            "session.tool",
        ] {
            assert!(describe_event(event).is_some(), "{event}");
        }
        for event in ["tick", "heartbeat", "presence", "health"] {
            assert!(describe_event(event).is_none(), "{event}");
        }
    }
}
