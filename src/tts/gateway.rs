//! `audio_pipeline = "gateway"`: synthetisiert über `tts.speak` gegen das
//! OpenClaw-Gateway statt dem lokalen Piper-Aufruf (siehe [`super::local`]).
//!
//! Anders als `chat.send` und `talk.session.*` ist `tts.speak` ein einfacher
//! synchroner Request/Response-Aufruf ohne Session-Lifecycle und ohne
//! Events - der Server liefert das fertige Audio (base64, Format laut
//! serverseitig konfiguriertem TTS-Provider - MP3, WAV, Opus, ... nicht fest
//! vorgegeben wie bei der G.711-mu-law-Transkriptionssession) direkt in der
//! Antwort zurück.

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use futures_util::SinkExt;
use serde_json::json;
use std::path::Path;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use crate::config::Config;
use crate::gateway::{connect_and_handshake, read_response};

/// Dateiendung, mit der die synthetisierte Audiodatei gespeichert wird,
/// wenn `tts.speak` keine `fileExtension` mitliefert. mp3 ist der
/// verbreitetste Standard-Ausgabecodec unter Cloud-TTS-Anbietern - `afplay`
/// erkennt das tatsächliche Format ohnehin am Dateiinhalt, die Endung ist
/// nur für Lesbarkeit/Diagnose relevant.
const TTS_SPEAK_FALLBACK_EXTENSION: &str = "mp3";

fn build_tts_speak_request(text: &str, request_id: &str) -> serde_json::Value {
    json!({
        "type": "req",
        "id": request_id,
        "method": "tts.speak",
        "params": {
            "text": text,
        }
    })
}

/// Leitet aus der optionalen `fileExtension`-Antwort (z. B. `".mp3"`) die
/// Dateiendung ohne führenden Punkt ab, mit Fallback auf
/// `TTS_SPEAK_FALLBACK_EXTENSION`.
fn resolve_tts_speak_extension(file_extension: Option<&str>) -> &str {
    match file_extension {
        Some(ext) => ext.trim_start_matches('.'),
        None => TTS_SPEAK_FALLBACK_EXTENSION,
    }
}

/// Synthetisiert `text` über das Gateway und spielt das Ergebnis über
/// `super::local::play_audio_file` ab (siehe Modul-Doku oben).
pub async fn synthesize_via_gateway(cfg: &Config, text: &str, tmp_dir: &Path) -> Result<()> {
    let mut ws = connect_and_handshake(cfg).await?;
    let timeout_secs = cfg.openclaw.timeout_secs;

    let request_id = uuid::Uuid::new_v4().to_string();
    let request = build_tts_speak_request(text, &request_id);
    ws.send(Message::Text(request.to_string()))
        .await
        .context("Kann tts.speak nicht senden")?;
    let response = read_response(&mut ws, timeout_secs, &request_id).await?;
    if response.ok != Some(true) {
        let error = response
            .error
            .context("tts.speak schlug fehl, ohne einen Fehler mitzuschicken")?;
        bail!(
            "tts.speak fehlgeschlagen ({}): {}",
            error.code,
            error.message
        );
    }
    let payload = response.payload.unwrap_or_default();
    let audio_base64 = payload
        .get("audioBase64")
        .and_then(|v| v.as_str())
        .context("tts.speak-Antwort enthielt kein audioBase64")?;
    let audio_bytes = BASE64_STANDARD
        .decode(audio_base64)
        .context("tts.speak lieferte ungültiges Base64")?;
    let extension =
        resolve_tts_speak_extension(payload.get("fileExtension").and_then(|v| v.as_str()));

    let out_path = tmp_dir.join(format!("gateway-tts-{}.{extension}", uuid::Uuid::new_v4()));
    tokio::fs::write(&out_path, &audio_bytes)
        .await
        .context("Kann synthetisierte Audiodatei nicht schreiben")?;
    info!(
        provider = ?payload.get("provider"),
        bytes = audio_bytes.len(),
        "tts.speak erfolgreich - spiele Antwort ab"
    );

    let play_result = super::local::play_audio_file(&cfg.tts, &out_path).await;
    if let Err(e) = tokio::fs::remove_file(&out_path).await {
        warn!(error = %e, path = %out_path.display(), "Konnte synthetisierte Audiodatei nicht löschen");
    }
    play_result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tts_speak_request_carries_only_the_text() {
        let req = build_tts_speak_request("Hallo Welt", "req-1");
        assert_eq!(req["method"], "tts.speak");
        assert_eq!(req["params"]["text"], "Hallo Welt");
        assert!(req["params"].get("voiceId").is_none());
    }

    #[test]
    fn tts_speak_extension_strips_the_leading_dot() {
        assert_eq!(resolve_tts_speak_extension(Some(".mp3")), "mp3");
        assert_eq!(resolve_tts_speak_extension(Some(".wav")), "wav");
    }

    #[test]
    fn tts_speak_extension_falls_back_when_absent() {
        assert_eq!(
            resolve_tts_speak_extension(None),
            TTS_SPEAK_FALLBACK_EXTENSION
        );
    }
}
