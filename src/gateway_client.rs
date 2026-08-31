//! Gateway-WebSocket-Client (0.2.0: read-only Streaming-Prototyp; 0.2.2:
//! `chat.send` für die volle Integration).
//!
//! Verbindet sich als OpenClaw-Operator mit dem Gateway, meldet sich über den
//! in `device_identity.rs` implementierten Ed25519-Signaturvertrag an und
//! abonniert die Transkript-/Nachrichtenereignisse des konfigurierten
//! Zielkanals. Zwei Nutzungsarten teilen sich denselben Connect-/
//! Subscribe-Ablauf:
//!   - `run_read_only_probe`: rein lesend, für `--probe-gateway`.
//!   - `send_chat_message`: löst über `chat.send` aktiv eine Antwort aus und
//!     sammelt die gestreamten `deltaText`-Events zur vollständigen Antwort
//!     (0.2.2, `transport = "websocket"`).
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
//!   - `packages/gateway-protocol/src/schema/logs-chat.ts`
//!     (`ChatSendParamsSchema`: `sessionKey` statt `key`, Pflichtfeld
//!     `idempotencyKey`; `ChatEventSchema`: `state` in
//!     `status`/`delta`/`final`/`aborted`/`error`, `deltaText`/`replace` nur
//!     bei `delta`).
//!   - `src/gateway/server-methods/chat-send-session.ts` (`clientRunId =
//!     p.idempotencyKey` - das vom Client vergebene `idempotencyKey` ist
//!     also *identisch* mit dem `runId`, das im ACK und in allen folgenden
//!     `chat`-Events steht. Kein zusätzlicher Antwort-Parse-Schritt nötig,
//!     um die eigenen Events herauszufiltern.)
//!   - `src/gateway/server-methods/chat-broadcast.ts` (`chat`-Events werden
//!     an `sessionKeys`-Topics gesendet, nicht an alle Verbindungen - ohne
//!     vorheriges `sessions.messages.subscribe` auf denselben Kanal kämen
//!     also gar keine Events an, selbst mit gültigem `chat.send`-ACK.)

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::path::Path;
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
        "talk.event" => Some("Talk-Ereignis (Transkription/Realtime)"),
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

/// Baut den `chat.send`-Request. Anders als `sessions.messages.subscribe`
/// heißt das Zielfeld hier `sessionKey`, nicht `key` -
/// `ChatSendParamsSchema` in `packages/gateway-protocol/src/schema/logs-chat.ts`
/// ist da unmissverständlich, auch wenn es inkonsistent zur
/// Subscribe-Methode ist. `idempotencyKey` ist Pflicht und wird 1:1 als
/// `runId` in ACK und Events zurückgespiegelt (siehe Modul-Doku oben).
fn build_chat_send_request(
    target_channel: &str,
    message: &str,
    idempotency_key: &str,
    request_id: &str,
) -> serde_json::Value {
    json!({
        "type": "req",
        "id": request_id,
        "method": "chat.send",
        "params": {
            "sessionKey": target_channel,
            "message": message,
            "idempotencyKey": idempotency_key,
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

/// Baut Identität, Verbindung und den signierten `connect`-Handshake auf und
/// gibt den verbundenen Socket zurück (nach erfolgreichem `hello-ok`).
/// Gemeinsamer erster Schritt für `run_read_only_probe` und
/// `send_chat_message` - beide brauchen exakt denselben Ablauf, bevor sie
/// sich in Abonnieren-und-Zuhören bzw. Abonnieren-und-Senden unterscheiden.
async fn connect_and_handshake(cfg: &Config) -> Result<WsStream> {
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

    Ok(ws)
}

/// Abonniert den konfigurierten Zielkanal auf einer bereits verbundenen
/// Session. Pflicht vor `chat.send`, nicht nur vor dem read-only Prototyp:
/// `chat`-Events werden vom Gateway nur an `sessionKeys`-Topics gesendet,
/// die die Verbindung abonniert hat (siehe Modul-Doku oben) - ohne dieses
/// Abonnement käme trotz erfolgreichem `chat.send`-ACK nie eine Antwort an.
async fn subscribe_channel(
    ws: &mut WsStream,
    target_channel: &str,
    timeout_secs: u64,
) -> Result<()> {
    let subscribe_id = uuid::Uuid::new_v4().to_string();
    let subscribe_req = build_subscribe_request(target_channel, &subscribe_id);
    ws.send(Message::Text(subscribe_req.to_string()))
        .await
        .context("Kann sessions.messages.subscribe nicht senden")?;
    let subscribe_res = read_response(ws, timeout_secs, &subscribe_id).await?;
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
    info!(%target_channel, "Zielkanal abonniert");
    Ok(())
}

/// Verbindet sich einmalig, meldet sich an, abonniert den konfigurierten
/// Zielkanal und protokolliert Events, bis der Prozess beendet wird (Strg+C)
/// oder die Verbindung abbricht.
pub async fn run_read_only_probe(cfg: &Config) -> Result<()> {
    let mut ws = connect_and_handshake(cfg).await?;
    let timeout_secs = cfg.openclaw.timeout_secs;
    subscribe_channel(&mut ws, &cfg.openclaw.target_channel, timeout_secs).await?;
    info!("Protokolliere eingehende Events (Strg+C zum Beenden)");

    // Events protokollieren, bis Strg+C oder Verbindungsende.
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

/// Sammelt eine `chat`-Antwort aus den `state: "delta"`-Events zu einem
/// `runId`. `replace: true` markiert laut Schema einen vollständigen
/// Refresh-Delta (kompletter Ersatz statt Anhängen) - kommt in der Praxis
/// selten vor, muss aber respektiert werden, sonst würde ein solcher Delta
/// den bisherigen Text duplizieren statt zu korrigieren.
#[derive(Debug, Default)]
struct ChatTextCollector {
    text: String,
}
impl ChatTextCollector {
    fn push_delta(&mut self, delta_text: &str, replace: bool) {
        if replace {
            self.text.clear();
        }
        self.text.push_str(delta_text);
    }

    fn into_response(self) -> String {
        self.text.trim().to_string()
    }
}

/// Wertet genau ein `chat`-Event für den beobachteten `run_id` aus.
/// `None` heißt "weiterlesen" (z. B. `status`-Zwischenstände oder Events zu
/// einem anderen, gleichzeitig laufenden `runId` auf demselben Kanal -
/// möglich, wenn derselbe Zielkanal auch von anderswo, etwa Telegram,
/// bespielt wird). `Some(Ok(..))`/`Some(Err(..))` beenden die Runde.
fn handle_chat_event(
    payload: &serde_json::Value,
    run_id: &str,
    collector: &mut ChatTextCollector,
) -> Option<Result<String>> {
    if payload.get("runId").and_then(|v| v.as_str()) != Some(run_id) {
        return None;
    }
    match payload.get("state").and_then(|v| v.as_str()) {
        Some("delta") => {
            let delta_text = payload
                .get("deltaText")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let replace = payload
                .get("replace")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            collector.push_delta(delta_text, replace);
            None
        }
        Some("final") => Some(Ok(std::mem::take(collector).into_response())),
        Some("aborted") => {
            let msg = payload
                .get("errorMessage")
                .and_then(|v| v.as_str())
                .unwrap_or("ohne Angabe eines Grundes");
            Some(Err(anyhow::anyhow!("chat.send wurde abgebrochen: {msg}")))
        }
        Some("error") => {
            let msg = payload
                .get("errorMessage")
                .and_then(|v| v.as_str())
                .unwrap_or("unbekannter Fehler");
            Some(Err(anyhow::anyhow!("chat.send-Fehler vom Gateway: {msg}")))
        }
        // "status" (Startphasen wie preparing_workspace) oder ein
        // unbekannter zukünftiger Zustand: nichts zu tun, weiterlesen.
        _ => None,
    }
}

/// Löst über `chat.send` eine Antwort im konfigurierten Zielkanal aus und
/// sammelt die gestreamten `deltaText`-Events zur vollständigen Antwort
/// (0.2.2, `transport = "websocket"` - Ersatz für `openclaw agent --json`).
///
/// `chat.send` ist laut Protokoll non-blocking: die Antwort auf den Request
/// selbst ist nur ein sofortiges ACK (`status: "started"`), die eigentliche
/// Antwort kommt über `chat`-Events auf dem zuvor abonnierten Kanal. Sobald
/// das ACK da ist, wird `on_ack` aufgerufen (typischerweise, um sofort eine
/// gesprochene Zwischenmeldung auszulösen, siehe
/// `OpenClawConfig::interim_message`) - erst danach wird auf die Events
/// gewartet, damit ein Fehlschlagen von `on_ack` selbst nicht den Empfang
/// der eigentlichen Antwort verzögert.
pub async fn send_chat_message<F, Fut>(cfg: &Config, message: &str, on_ack: F) -> Result<String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ()>,
{
    let mut ws = connect_and_handshake(cfg).await?;
    let timeout_secs = cfg.openclaw.timeout_secs;
    subscribe_channel(&mut ws, &cfg.openclaw.target_channel, timeout_secs).await?;

    // Das selbst vergebene idempotencyKey ist laut Gateway-Quellcode
    // identisch mit dem runId, das ACK und Events tragen (siehe Modul-Doku
    // oben) - es muss also nicht erst aus der ACK-Antwort gelesen werden.
    let idempotency_key = uuid::Uuid::new_v4().to_string();
    let send_id = uuid::Uuid::new_v4().to_string();
    let send_req = build_chat_send_request(
        &cfg.openclaw.target_channel,
        message,
        &idempotency_key,
        &send_id,
    );
    ws.send(Message::Text(send_req.to_string()))
        .await
        .context("Kann chat.send nicht senden")?;

    let ack = read_response(&mut ws, timeout_secs, &send_id).await?;
    if ack.ok != Some(true) {
        let error = ack
            .error
            .context("chat.send schlug fehl, ohne einen Fehler mitzuschicken")?;
        bail!(
            "chat.send fehlgeschlagen ({}): {}",
            error.code,
            error.message
        );
    }
    info!(run_id = %idempotency_key, "chat.send bestätigt (ACK) - warte auf gestreamte Antwort");

    on_ack().await;

    let mut collector = ChatTextCollector::default();
    loop {
        let frame = read_frame(&mut ws, timeout_secs).await?;
        if frame.frame_type != "event" || frame.event.as_deref() != Some("chat") {
            continue;
        }
        let Some(payload) = frame.payload else {
            continue;
        };
        if let Some(outcome) = handle_chat_event(&payload, &idempotency_key, &mut collector) {
            return outcome;
        }
    }
}

/// Gateway-seitig fest vorgegebenes Eingabeformat für die Talk-
/// Transkriptions-Session - keine freie Wahl, siehe `assertRelayInputAudioConfig`
/// in `src/gateway/talk-transcription-relay.ts`: der Server lehnt jede
/// andere Kombination beim `talk.session.create` ab. Ausdrücklich NICHT
/// PCM16/24kHz, wie die (selbst als ungeprüft markierte) Recherchegrundlage
/// `Gateway-Transcription.md` annahm - das ist das Format des separaten
/// Realtime-Voice-Pfads (`mode: "realtime"`), nicht des
/// Transkriptions-Pfads (`mode: "transcription"`).
const TALK_EXPECTED_AUDIO_ENCODING: &str = "g711_ulaw";
const TALK_EXPECTED_AUDIO_SAMPLE_RATE_HZ: u64 = 8000;
/// ~0,5s Audio pro Chunk bei 8000 Byte/s (1 Byte/Sample, mono, 8kHz mu-law) -
/// klein genug, um laut Doku nicht unbegrenzt Speicher zu puffern, groß
/// genug, um nicht bei jedem `appendAudio` nur wenige Bytes zu verschicken.
const TALK_AUDIO_CHUNK_BYTES: usize = 4000;

fn build_talk_session_create_request(request_id: &str) -> serde_json::Value {
    json!({
        "type": "req",
        "id": request_id,
        "method": "talk.session.create",
        "params": {
            "mode": "transcription",
            "transport": "gateway-relay",
            "brain": "none",
        }
    })
}

fn build_talk_append_audio_request(
    session_id: &str,
    audio_base64: &str,
    request_id: &str,
) -> serde_json::Value {
    json!({
        "type": "req",
        "id": request_id,
        "method": "talk.session.appendAudio",
        "params": {
            "sessionId": session_id,
            "audioBase64": audio_base64,
        }
    })
}

fn build_talk_session_close_request(session_id: &str, request_id: &str) -> serde_json::Value {
    json!({
        "type": "req",
        "id": request_id,
        "method": "talk.session.close",
        "params": {
            "sessionId": session_id,
        }
    })
}

/// Sammelt die Transkript-Segmente einer Talk-Session. Ein
/// Streaming-STT-Provider kann mehrere `transcript`-Events pro Session
/// liefern (z. B. mehrere Sprechpausen innerhalb einer Aufnahme) -
/// nicht-leere Segmente werden deshalb aneinandergehängt statt einander zu
/// ersetzen.
#[derive(Debug, Default)]
struct TalkTranscriptCollector {
    text: String,
}
impl TalkTranscriptCollector {
    fn push_transcript(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if !self.text.is_empty() {
            self.text.push(' ');
        }
        self.text.push_str(text);
    }

    fn into_transcript(self) -> String {
        self.text.trim().to_string()
    }
}

/// Wertet genau ein `talk.event` für die beobachtete `transcriptionSessionId`
/// aus. `None` heißt "weiterlesen" - auch nach einem eingesammelten
/// `transcript`, da eine Session mehrere davon liefern kann. `Some(Ok(()))`
/// markiert das `close`-Event (Session sauber beendet, keine weiteren
/// Transkripte mehr zu erwarten), `Some(Err(..))` einen Provider-/
/// Session-Fehler.
fn handle_talk_event(
    payload: &serde_json::Value,
    session_id: &str,
    collector: &mut TalkTranscriptCollector,
) -> Option<Result<()>> {
    if payload
        .get("transcriptionSessionId")
        .and_then(|v| v.as_str())
        != Some(session_id)
    {
        return None;
    }
    match payload.get("type").and_then(|v| v.as_str()) {
        Some("transcript") => {
            if let Some(text) = payload.get("text").and_then(|v| v.as_str()) {
                collector.push_transcript(text);
            }
            None
        }
        Some("error") => {
            let msg = payload
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unbekannter Fehler");
            Some(Err(anyhow::anyhow!("Gateway-Transkriptionsfehler: {msg}")))
        }
        Some("close") => Some(Ok(())),
        // "ready"/"inputAudio"/"partial"/"speechStart" oder ein unbekannter
        // zukünftiger Typ: nichts zu tun, weiterlesen. Zwischenergebnisse
        // (`partial`) fließen bewusst nicht in den Collector ein - nur
        // `transcript` liefert laut Server-Quellcode tatsächlich fertigen
        // Text, `partial` ist reines Zwischenfeedback, das später vom
        // finalen `transcript` überschrieben/ersetzt wird.
        _ => None,
    }
}

/// Wartet auf die Antwort mit `id == expected_id`, verarbeitet dabei aber
/// zwischenzeitlich ankommende `talk.event`-Frames für `session_id` in
/// `collector` statt sie wie `read_response` stillschweigend zu verwerfen -
/// der STT-Provider kann Transkripte liefern, bevor der nächste
/// `appendAudio`/`close` überhaupt bestätigt ist.
async fn await_talk_response_collecting_events(
    ws: &mut WsStream,
    timeout_secs: u64,
    expected_id: &str,
    session_id: &str,
    collector: &mut TalkTranscriptCollector,
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
        if frame.frame_type == "event" && frame.event.as_deref() == Some("talk.event") {
            if let Some(payload) = &frame.payload {
                if let Some(Err(e)) = handle_talk_event(payload, session_id, collector) {
                    return Err(e);
                }
            }
        }
    }
}

/// Wartet nach dem eigenen `talk.session.close` auf das abschließende
/// `close`-Event der Session (der Server hält die Session laut Quellcode
/// noch bis zu einige Sekunden offen, um ein letztes Transkript
/// nachzuliefern, bevor er sie endgültig beendet).
async fn wait_for_talk_session_close(
    ws: &mut WsStream,
    timeout_secs: u64,
    session_id: &str,
    collector: &mut TalkTranscriptCollector,
) -> Result<()> {
    loop {
        let frame = read_frame(ws, timeout_secs).await?;
        if frame.frame_type != "event" || frame.event.as_deref() != Some("talk.event") {
            continue;
        }
        let Some(payload) = &frame.payload else {
            continue;
        };
        if let Some(outcome) = handle_talk_event(payload, session_id, collector) {
            return outcome;
        }
    }
}

/// Transkribiert `wav_path` über eine Gateway-Talk-Session
/// (`audio_pipeline = "gateway"` - Ersatz für ffmpeg-Normalisierung +
/// `whisper-cli`, siehe `transcribe.rs`). Ablauf laut tatsächlichem
/// OpenClaw-Quellcode (`talk-session.ts`, `talk-transcription-relay.ts`),
/// nicht nur laut `Gateway-Transcription.md` (die als ungeprüfte
/// Recherchegrundlage markiert ist und beim Audioformat nachweislich
/// falsch lag, siehe `TALK_EXPECTED_AUDIO_ENCODING`):
///   `talk.session.create` (mode=transcription, transport=gateway-relay,
///   brain=none) -> Audio zu G.711 mu-law/8kHz konvertieren -> in Chunks
///   per `talk.session.appendAudio` senden -> `talk.session.close` ->
///   auf das abschließende `talk.event` warten. Anders als bei `chat.send`
///   ist **kein** vorheriges `sessions.messages.subscribe` nötig: die
///   Events gehen direkt an die anfragende Verbindung
///   (`context.broadcastToConnIds`), nicht an ein Session-Key-Topic.
pub async fn transcribe_via_gateway(cfg: &Config, wav_path: &Path) -> Result<String> {
    let mut ws = connect_and_handshake(cfg).await?;
    let timeout_secs = cfg.openclaw.timeout_secs;

    let create_id = uuid::Uuid::new_v4().to_string();
    let create_req = build_talk_session_create_request(&create_id);
    ws.send(Message::Text(create_req.to_string()))
        .await
        .context("Kann talk.session.create nicht senden")?;
    let create_res = read_response(&mut ws, timeout_secs, &create_id).await?;
    if create_res.ok != Some(true) {
        let error = create_res
            .error
            .context("talk.session.create schlug fehl, ohne einen Fehler mitzuschicken")?;
        bail!(
            "talk.session.create fehlgeschlagen ({}): {}",
            error.code,
            error.message
        );
    }
    let create_payload = create_res.payload.unwrap_or_default();
    let session_id = create_payload
        .get("sessionId")
        .and_then(|v| v.as_str())
        .context("talk.session.create-Antwort enthielt keine sessionId")?
        .to_string();

    let audio = create_payload.get("audio");
    let encoding = audio
        .and_then(|a| a.get("inputEncoding"))
        .and_then(|v| v.as_str());
    let sample_rate = audio
        .and_then(|a| a.get("inputSampleRateHz"))
        .and_then(|v| v.as_u64());
    if encoding != Some(TALK_EXPECTED_AUDIO_ENCODING)
        || sample_rate != Some(TALK_EXPECTED_AUDIO_SAMPLE_RATE_HZ)
    {
        bail!(
            "Gateway erwartet ein anderes Audioformat als angenommen (bekommen: {encoding:?}/{sample_rate:?}Hz, \
             erwartet: {TALK_EXPECTED_AUDIO_ENCODING}/{TALK_EXPECTED_AUDIO_SAMPLE_RATE_HZ}Hz) - \
             audio_pipeline = \"gateway\" muss gegen den aktuellen OpenClaw-Stand neu geprüft werden."
        );
    }
    info!(%session_id, "Talk-Transkriptionssession erstellt");

    let mulaw_path = wav_path.with_extension("mulaw");
    crate::transcribe::convert_to_gateway_mulaw(&cfg.general, wav_path, &mulaw_path, timeout_secs)
        .await?;
    let audio_bytes = tokio::fs::read(&mulaw_path)
        .await
        .context("Kann konvertierte Audiodatei nicht lesen")?;
    if let Err(e) = tokio::fs::remove_file(&mulaw_path).await {
        warn!(error = %e, path = %mulaw_path.display(), "Konnte konvertierte Audiodatei nicht löschen");
    }

    let mut collector = TalkTranscriptCollector::default();
    for chunk in audio_bytes.chunks(TALK_AUDIO_CHUNK_BYTES) {
        let audio_base64 = BASE64_STANDARD.encode(chunk);
        let append_id = uuid::Uuid::new_v4().to_string();
        let append_req = build_talk_append_audio_request(&session_id, &audio_base64, &append_id);
        ws.send(Message::Text(append_req.to_string()))
            .await
            .context("Kann talk.session.appendAudio nicht senden")?;
        let append_res = await_talk_response_collecting_events(
            &mut ws,
            timeout_secs,
            &append_id,
            &session_id,
            &mut collector,
        )
        .await?;
        if append_res.ok != Some(true) {
            let error = append_res
                .error
                .context("talk.session.appendAudio schlug fehl, ohne einen Fehler mitzuschicken")?;
            bail!(
                "talk.session.appendAudio fehlgeschlagen ({}): {}",
                error.code,
                error.message
            );
        }
    }

    let close_id = uuid::Uuid::new_v4().to_string();
    let close_req = build_talk_session_close_request(&session_id, &close_id);
    ws.send(Message::Text(close_req.to_string()))
        .await
        .context("Kann talk.session.close nicht senden")?;
    let close_res = await_talk_response_collecting_events(
        &mut ws,
        timeout_secs,
        &close_id,
        &session_id,
        &mut collector,
    )
    .await?;
    if close_res.ok != Some(true) {
        let error = close_res
            .error
            .context("talk.session.close schlug fehl, ohne einen Fehler mitzuschicken")?;
        bail!(
            "talk.session.close fehlgeschlagen ({}): {}",
            error.code,
            error.message
        );
    }

    wait_for_talk_session_close(&mut ws, timeout_secs, &session_id, &mut collector).await?;

    Ok(collector.into_transcript())
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
            "talk.event",
        ] {
            assert!(describe_event(event).is_some(), "{event}");
        }
        for event in ["tick", "heartbeat", "presence", "health"] {
            assert!(describe_event(event).is_none(), "{event}");
        }
    }

    /// Regression: `chat.send` erwartet `sessionKey`, nicht `key` wie
    /// `sessions.messages.subscribe` - dieselbe Verwechslung, die schon beim
    /// `client.id`/`mode`-Bugfix aufgefallen ist (Feldnamen zwischen
    /// Gateway-Methoden nicht aus Analogie annehmen, sondern je Methode am
    /// tatsächlichen Schema prüfen).
    #[test]
    fn chat_send_request_uses_session_key_not_key() {
        let req = build_chat_send_request("agent:main:voice-assistant", "Hallo", "idem-1", "req-1");
        assert_eq!(req["method"], "chat.send");
        assert_eq!(req["params"]["sessionKey"], "agent:main:voice-assistant");
        assert_eq!(req["params"]["message"], "Hallo");
        assert_eq!(req["params"]["idempotencyKey"], "idem-1");
        assert!(req["params"].get("key").is_none());
    }

    #[test]
    fn chat_text_collector_appends_deltas_in_order() {
        let mut collector = ChatTextCollector::default();
        collector.push_delta("Es ist ", false);
        collector.push_delta("kurz nach acht.", false);
        assert_eq!(collector.into_response(), "Es ist kurz nach acht.");
    }

    /// `replace: true` markiert laut `ChatDeltaEventSchema` einen
    /// vollständigen Refresh-Delta - der bisherige Text muss dabei ersetzt,
    /// nicht mit dem neuen zusammengehängt werden.
    #[test]
    fn chat_text_collector_replace_delta_discards_previous_text() {
        let mut collector = ChatTextCollector::default();
        collector.push_delta("Vorläufiger Entwurf", false);
        collector.push_delta("Endgültiger Text.", true);
        assert_eq!(collector.into_response(), "Endgültiger Text.");
    }

    #[test]
    fn chat_text_collector_trims_the_final_response() {
        let mut collector = ChatTextCollector::default();
        collector.push_delta("  Hallo Welt  \n", false);
        assert_eq!(collector.into_response(), "Hallo Welt");
    }

    #[test]
    fn handle_chat_event_ignores_events_for_a_different_run_id() {
        let mut collector = ChatTextCollector::default();
        let payload = json!({"runId": "other-run", "state": "final"});
        assert!(handle_chat_event(&payload, "my-run", &mut collector).is_none());
    }

    #[test]
    fn handle_chat_event_collects_deltas_and_returns_on_final() {
        let mut collector = ChatTextCollector::default();
        let delta = json!({"runId": "run-1", "state": "delta", "deltaText": "Hallo"});
        assert!(handle_chat_event(&delta, "run-1", &mut collector).is_none());
        let delta2 = json!({"runId": "run-1", "state": "delta", "deltaText": " Welt"});
        assert!(handle_chat_event(&delta2, "run-1", &mut collector).is_none());

        let done = json!({"runId": "run-1", "state": "final"});
        let outcome = handle_chat_event(&done, "run-1", &mut collector).expect("final beendet");
        assert_eq!(outcome.unwrap(), "Hallo Welt");
    }

    #[test]
    fn handle_chat_event_ignores_status_events() {
        let mut collector = ChatTextCollector::default();
        let status = json!({"runId": "run-1", "state": "status", "phase": "preparing_workspace"});
        assert!(handle_chat_event(&status, "run-1", &mut collector).is_none());
    }

    #[test]
    fn handle_chat_event_turns_aborted_into_an_error() {
        let mut collector = ChatTextCollector::default();
        let aborted =
            json!({"runId": "run-1", "state": "aborted", "errorMessage": "Nutzerabbruch"});
        let outcome =
            handle_chat_event(&aborted, "run-1", &mut collector).expect("aborted beendet");
        assert!(outcome.is_err());
        assert!(outcome.unwrap_err().to_string().contains("Nutzerabbruch"));
    }

    #[test]
    fn handle_chat_event_turns_error_state_into_an_error() {
        let mut collector = ChatTextCollector::default();
        let error_event =
            json!({"runId": "run-1", "state": "error", "errorMessage": "Modell nicht erreichbar"});
        let outcome =
            handle_chat_event(&error_event, "run-1", &mut collector).expect("error beendet");
        assert!(outcome.is_err());
        assert!(outcome
            .unwrap_err()
            .to_string()
            .contains("Modell nicht erreichbar"));
    }

    #[test]
    fn talk_session_create_request_uses_the_transcription_gateway_relay_combination() {
        let req = build_talk_session_create_request("req-1");
        assert_eq!(req["method"], "talk.session.create");
        assert_eq!(req["params"]["mode"], "transcription");
        assert_eq!(req["params"]["transport"], "gateway-relay");
        assert_eq!(req["params"]["brain"], "none");
    }

    #[test]
    fn talk_append_audio_request_uses_session_id_and_audio_base64() {
        let req = build_talk_append_audio_request("session-1", "aGFsbG8=", "req-2");
        assert_eq!(req["method"], "talk.session.appendAudio");
        assert_eq!(req["params"]["sessionId"], "session-1");
        assert_eq!(req["params"]["audioBase64"], "aGFsbG8=");
    }

    #[test]
    fn talk_session_close_request_carries_only_the_session_id() {
        let req = build_talk_session_close_request("session-1", "req-3");
        assert_eq!(req["method"], "talk.session.close");
        assert_eq!(req["params"]["sessionId"], "session-1");
        assert!(req["params"].get("audioBase64").is_none());
    }

    #[test]
    fn talk_transcript_collector_joins_multiple_segments_with_a_space() {
        let mut collector = TalkTranscriptCollector::default();
        collector.push_transcript("Hallo");
        collector.push_transcript("Welt");
        assert_eq!(collector.into_transcript(), "Hallo Welt");
    }

    #[test]
    fn talk_transcript_collector_ignores_empty_segments() {
        let mut collector = TalkTranscriptCollector::default();
        collector.push_transcript("Hallo");
        collector.push_transcript("   ");
        collector.push_transcript("Welt");
        assert_eq!(collector.into_transcript(), "Hallo Welt");
    }

    #[test]
    fn handle_talk_event_ignores_events_for_a_different_session() {
        let mut collector = TalkTranscriptCollector::default();
        let payload = json!({"transcriptionSessionId": "other-session", "type": "close"});
        assert!(handle_talk_event(&payload, "my-session", &mut collector).is_none());
    }

    #[test]
    fn handle_talk_event_collects_transcript_text_and_keeps_reading() {
        let mut collector = TalkTranscriptCollector::default();
        let event = json!({
            "transcriptionSessionId": "session-1",
            "type": "transcript",
            "text": "Wie spät ist es?",
            "final": true,
        });
        assert!(handle_talk_event(&event, "session-1", &mut collector).is_none());
        assert_eq!(collector.into_transcript(), "Wie spät ist es?");
    }

    /// Regression-Anker: `partial` darf NICHT in den Collector einfließen -
    /// nur `transcript` liefert laut Server-Quellcode fertigen Text.
    #[test]
    fn handle_talk_event_ignores_partial_transcripts() {
        let mut collector = TalkTranscriptCollector::default();
        let partial = json!({
            "transcriptionSessionId": "session-1",
            "type": "partial",
            "text": "Wie spät",
        });
        assert!(handle_talk_event(&partial, "session-1", &mut collector).is_none());
        assert_eq!(collector.into_transcript(), "");
    }

    #[test]
    fn handle_talk_event_close_ends_the_session_without_error() {
        let mut collector = TalkTranscriptCollector::default();
        let close =
            json!({"transcriptionSessionId": "session-1", "type": "close", "reason": "completed"});
        let outcome =
            handle_talk_event(&close, "session-1", &mut collector).expect("close beendet");
        assert!(outcome.is_ok());
    }

    #[test]
    fn handle_talk_event_turns_error_type_into_an_error() {
        let mut collector = TalkTranscriptCollector::default();
        let error_event = json!({
            "transcriptionSessionId": "session-1",
            "type": "error",
            "message": "Provider nicht erreichbar",
        });
        let outcome =
            handle_talk_event(&error_event, "session-1", &mut collector).expect("error beendet");
        assert!(outcome.is_err());
        assert!(outcome
            .unwrap_err()
            .to_string()
            .contains("Provider nicht erreichbar"));
    }
}
