//! `transport = "websocket"`: spricht direkt mit dem Gateway statt einen
//! CLI-Subprozess zu starten. Zwei Nutzungsarten teilen sich denselben
//! Connect-/Subscribe-Ablauf aus `crate::gateway`:
//!   - [`run_read_only_probe`]: rein lesend, für `--probe-gateway`.
//!   - [`send_chat_message`]: löst über `chat.send` aktiv eine Antwort aus
//!     und sammelt die gestreamten `deltaText`-Events zur vollständigen
//!     Antwort (0.2.2, volle Integration).
//!
//! Feldnamen gegen den tatsächlichen OpenClaw-Quellcode geprüft:
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
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::gateway::{
    connect_and_handshake, describe_event, read_frame, read_response, subscribe_channel,
    InboundFrame,
};

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
/// Antwort kommt über `chat`-Events auf dem zuvor abonnierten Kanal. Die
/// Bridge bleibt bis dahin einfach still - dieselbe Wartezeit, die auch der
/// synchrone CLI-Aufruf hätte, nur ohne Zwischenmeldung (0.2.2 hatte hier
/// versuchsweise ein ACK-getriggertes "Ich schau mir das an", das sich im
/// Feldtest als unerwünscht herausstellte und in 0.2.6 wieder entfernt
/// wurde, siehe CHANGELOG.md).
pub async fn send_chat_message(cfg: &Config, message: &str) -> Result<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
