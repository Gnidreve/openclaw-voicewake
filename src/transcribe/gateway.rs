//! `audio_pipeline = "gateway"`: transkribiert über eine
//! OpenClaw-Gateway-Talk-Session statt lokalem `whisper-cli` (siehe
//! [`super::local`]). Ablauf laut tatsächlichem OpenClaw-Quellcode
//! (`talk-session.ts`, `talk-transcription-relay.ts`), nicht nur laut
//! `Gateway-Transcription.md` (die als ungeprüfte Recherchegrundlage
//! markiert ist und beim Audioformat nachweislich falsch lag, siehe
//! `TALK_EXPECTED_AUDIO_ENCODING`):
//!   `talk.session.create` (mode=transcription, transport=gateway-relay,
//!   brain=none) -> Audio zu G.711 mu-law/8kHz konvertieren (per
//!   `super::local::convert_to_gateway_mulaw`) -> in Chunks per
//!   `talk.session.appendAudio` senden -> `talk.session.close` -> auf das
//!   abschließende `talk.event` warten. Anders als bei `chat.send` ist
//!   **kein** vorheriges `sessions.messages.subscribe` nötig: die Events
//!   gehen direkt an die anfragende Verbindung
//!   (`context.broadcastToConnIds`), nicht an ein Session-Key-Topic.

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use futures_util::SinkExt;
use serde_json::json;
use std::path::Path;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use crate::config::Config;
use crate::gateway::{connect_and_handshake, read_frame, read_response, InboundFrame, WsStream};

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

/// Transkribiert `wav_path` über eine Gateway-Talk-Session (siehe
/// Modul-Doku oben).
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
    super::local::convert_to_gateway_mulaw(&cfg.general, wav_path, &mulaw_path, timeout_secs)
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
