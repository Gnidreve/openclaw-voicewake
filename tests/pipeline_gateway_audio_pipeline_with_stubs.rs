//! Integrationstest für `audio_pipeline = "gateway"` (0.2.5 Transkription +
//! 0.2.7 Sprachausgabe über das OpenClaw-Gateway) gegen ein selbstgebautes
//! Mock-Gateway - das WebSocket-Pendant zu `tests/pipeline_with_stubs.rs`,
//! diesmal für beide Enden der Runde statt nur die Antwortseite.
//!
//! `audio_pipeline = "gateway"` setzt `transport = "websocket"` voraus
//! (siehe `Config::validate`) und ersetzt sowohl die Transkription als auch
//! die Sprachausgabe - eine Runde öffnet deshalb drei komplett getrennte
//! Gateway-Verbindungen nacheinander: eine für `talk.session.create` ->
//! `appendAudio` -> `close` (Transkription), eine für `chat.send`
//! (Antworttext), eine für `tts.speak` (Antwort synthetisieren).
//! `transcribe_via_gateway`, `send_chat_message` und
//! `synthesize_via_gateway` verbinden sich alle unabhängig neu (siehe
//! `gateway_client.rs`). Das Mock-Gateway hier nimmt deshalb drei
//! Verbindungen nacheinander an und behandelt sie nach der zuerst
//! empfangenen Methode.
//!
//! ffmpeg wird durch einen Stub ersetzt, der unabhängig vom echten
//! Audioinhalt einen festen Byte-String schreibt - der Test prüft, dass
//! genau diese (base64-kodierten) Bytes unverändert bei
//! `talk.session.appendAudio` ankommen. Der Player-Stub (`afplay`-Ersatz)
//! schreibt umgekehrt mit, welchen Dateiinhalt er abgespielt bekommen hat -
//! damit lässt sich prüfen, dass genau das von `tts.speak` gelieferte
//! (base64-dekodierte) Audio unverändert bei der Wiedergabe ankommt. Piper
//! selbst wird bei `audio_pipeline = "gateway"` gar nicht mehr aufgerufen,
//! deshalb gibt es hier keinen Piper-Stub.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

/// Siehe `tests/pipeline_with_stubs.rs` - dieselbe feste, nicht
/// konfigurierbare Einzelinstanz-Sperre macht parallele Bridge-Starts aus
/// unterschiedlichen Testfiles unsicher.
static SEQUENTIAL_BINARY_RUNS: Mutex<()> = Mutex::new(());

#[cfg(unix)]
fn write_stub(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, body).expect("Stub schreiben");
    let mut perms = std::fs::metadata(&path)
        .expect("Stub-Metadaten")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("Stub ausführbar machen");
    path
}

/// Ignoriert den tatsächlichen Audioinhalt und schreibt immer denselben
/// festen Byte-String - genug, um die base64-Kodierung/Chunk-Übertragung zu
/// prüfen, ohne echtes Audio zu brauchen. Muss mit
/// `build_ffmpeg_mulaw_args` (`-nostdin -y -i <in> -ac 1 -ar 8000 -f mulaw
/// <out>`) zurechtkommen.
const FFMPEG_MULAW_STUB: &str = r#"#!/bin/sh
set -eu
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    -nostdin|-y) shift ;;
    -i|-ac|-ar|-f) shift 2 ;;
    *) out="$1"; shift ;;
  esac
done
[ -n "$out" ] || { echo "kein Ausgabepfad" >&2; exit 3; }
printf 'FAKE-MULAW-AUDIO-BYTES' > "$out"
"#;

#[cfg(unix)]
#[tokio::test]
async fn gateway_audio_pipeline_transcribes_and_speaks_entirely_via_the_gateway() {
    let result = tokio::time::timeout(Duration::from_secs(20), run_test()).await;
    result.expect("Test ist hängen geblieben - vermutlich ein Deadlock im talk.session-Ablauf");
}

async fn run_test() {
    let dir = std::env::temp_dir().join(format!(
        "voicebridge-gateway-audio-pipeline-{}",
        uuid::Uuid::new_v4()
    ));
    let bin_dir = dir.join("bin");
    std::fs::create_dir_all(&bin_dir).expect("Testverzeichnis anlegen");

    let ffmpeg = write_stub(&bin_dir, "ffmpeg-stub", FFMPEG_MULAW_STUB);

    // Schreibt mit, welche Datei mit welchem Inhalt "abgespielt" wurde -
    // damit lässt sich prüfen, dass genau das von tts.speak gelieferte
    // (base64-dekodierte) Audio unverändert bei der Wiedergabe ankommt.
    let played_log = dir.join("played.log");
    let player = write_stub(
        &bin_dir,
        "player-stub",
        &format!(
            r#"#!/bin/sh
set -eu
cat "$1" >> "{played_log}"
exit 0
"#,
            played_log = played_log.display(),
        ),
    );

    let sample = dir.join("sample.wav");
    std::fs::write(&sample, b"RIFF").expect("Beispieldatei schreiben");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Mock-Gateway sollte einen Port belegen können");
    let port = listener.local_addr().unwrap().port();
    let target_channel = "agent:main:voice-assistant";
    let expected_final_response = "Es ist kurz nach acht.";

    let server = tokio::spawn(run_mock_gateway(
        listener,
        target_channel.to_string(),
        expected_final_response.to_string(),
    ));

    let chat_log = dir.join("chat.log");
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!(
            r#"
[openclaw]
target_channel = "{target_channel}"
transport = "websocket"
audio_pipeline = "gateway"
gateway_host = "127.0.0.1"
gateway_port = {port}

[tts]
player_binary = "{player}"

[sound]
enabled = false

[general]
ffmpeg_binary = "{ffmpeg}"
temp_dir = "{tmp}"

[transcription_log]
path = "{chat_log}"
"#,
            player = player.display(),
            ffmpeg = ffmpeg.display(),
            tmp = dir.display(),
            chat_log = chat_log.display(),
        ),
    )
    .expect("Konfiguration schreiben");

    let fake_home = dir.join("home");
    std::fs::create_dir_all(&fake_home).expect("Fake-HOME anlegen");

    let config_path = config.clone();
    let home_path = fake_home.clone();
    let client = tokio::task::spawn_blocking(move || {
        let _guard = SEQUENTIAL_BINARY_RUNS.lock().unwrap();
        Command::new(env!("CARGO_BIN_EXE_openclaw-voicebridge"))
            .args([
                "--config",
                config_path.to_str().unwrap(),
                "--dry-run",
                "--dry-run-file",
                sample.to_str().unwrap(),
                "--once",
            ])
            .env("HOME", &home_path)
            .output()
    });

    let output = client
        .await
        .expect("Task sollte nicht abbrechen")
        .expect("Bridge starten");
    server
        .await
        .expect("Mock-Gateway-Task sollte nicht abbrechen")
        .expect("Mock-Gateway sollte den Ablauf ohne Assertion-Fehler abschließen");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let log = std::fs::read_to_string(&chat_log).unwrap_or_default();
    let played = std::fs::read_to_string(&played_log).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);

    // Das Transkript kommt hier aus dem Mock-Gateway (`talk.event`), nicht
    // aus whisper-cli - `Wie spät ist es?` ist bewusst der Text, den das
    // Mock-Gateway als `transcript`-Event zurückgibt.
    assert!(
        log.contains(r#"[Input] "Wie spät ist es?""#),
        "über talk.session gelieferte Transkription fehlt im Log.\nLog:\n{log}\nstderr:\n{stderr}"
    );
    assert!(
        log.contains(&format!(r#"[Output] "{expected_final_response}""#)),
        "aus deltaText zusammengesetzte Antwort fehlt im Log.\nLog:\n{log}\nstderr:\n{stderr}"
    );
    // Bestätigt, dass tts.speak tatsächlich aufgerufen und sein Audio
    // unverändert an die Wiedergabe weitergereicht wurde - kein lokaler
    // Piper-Aufruf mehr bei audio_pipeline = "gateway".
    assert_eq!(
        played, "FAKE-TTS-AUDIO-BYTES",
        "das von tts.speak gelieferte Audio wurde nicht unverändert abgespielt.\nstderr:\n{stderr}"
    );
}

/// Nimmt drei Verbindungen nacheinander an: die erste für die
/// Talk-Transkriptionssession, die zweite für `chat.send`, die dritte für
/// `tts.speak` - siehe Modul-Doku oben, warum es getrennte Verbindungen
/// sind.
async fn run_mock_gateway(
    listener: TcpListener,
    expected_target_channel: String,
    final_response: String,
) -> Result<(), String> {
    handle_talk_transcription_connection(&listener).await?;
    handle_chat_send_connection(&listener, &expected_target_channel, &final_response).await?;
    handle_tts_speak_connection(&listener, &final_response).await?;
    Ok(())
}

async fn accept_and_handshake(
    listener: &TcpListener,
) -> Result<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, String> {
    let (stream, _) = listener
        .accept()
        .await
        .map_err(|e| format!("Bridge sollte sich verbinden: {e}"))?;
    let mut ws = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(|e| format!("WebSocket-Handshake sollte gelingen: {e}"))?;

    let challenge = serde_json::json!({
        "type": "event",
        "event": "connect.challenge",
        "payload": {"nonce": "test-nonce", "ts": 1_700_000_000_000i64},
    });
    send(&mut ws, &challenge).await?;

    let connect_req = recv_json(&mut ws).await?;
    if connect_req["method"] != "connect" {
        return Err(format!("erwartet method=connect, bekam {connect_req:?}"));
    }
    let scopes = connect_req["params"]["scopes"].clone();
    let hello_ok = serde_json::json!({
        "type": "res",
        "id": connect_req["id"],
        "ok": true,
        "payload": {
            "type": "hello-ok",
            "protocol": 4,
            "server": {"version": "test", "connId": "conn-1"},
            "features": {"methods": [], "events": []},
            "snapshot": {},
            "auth": {"role": "operator", "scopes": scopes},
            "policy": {"maxPayload": 1000000, "maxBufferedBytes": 1000000, "tickIntervalMs": 15000},
        }
    });
    send(&mut ws, &hello_ok).await?;
    Ok(ws)
}

async fn handle_talk_transcription_connection(listener: &TcpListener) -> Result<(), String> {
    let mut ws = accept_and_handshake(listener).await?;

    let create_req = recv_json(&mut ws).await?;
    if create_req["method"] != "talk.session.create" {
        return Err(format!(
            "erwartet method=talk.session.create, bekam {create_req:?}"
        ));
    }
    if create_req["params"]["mode"] != "transcription"
        || create_req["params"]["transport"] != "gateway-relay"
        || create_req["params"]["brain"] != "none"
    {
        return Err(format!(
            "falsche talk.session.create-Parameter: {:?}",
            create_req["params"]
        ));
    }
    let session_id = "talk-session-1";
    let create_ok = serde_json::json!({
        "type": "res",
        "id": create_req["id"],
        "ok": true,
        "payload": {
            "provider": "test",
            "mode": "transcription",
            "transport": "gateway-relay",
            "sessionId": session_id,
            "audio": {"inputEncoding": "g711_ulaw", "inputSampleRateHz": 8000},
            "expiresAt": 9_999_999_999i64,
        },
    });
    send(&mut ws, &create_ok).await?;

    let append_req = recv_json(&mut ws).await?;
    if append_req["method"] != "talk.session.appendAudio" {
        return Err(format!(
            "erwartet method=talk.session.appendAudio, bekam {append_req:?}"
        ));
    }
    if append_req["params"]["sessionId"] != session_id {
        return Err(format!(
            "falsche sessionId bei appendAudio: {:?}",
            append_req["params"]["sessionId"]
        ));
    }
    let audio_base64 = append_req["params"]["audioBase64"]
        .as_str()
        .ok_or("audioBase64 fehlte bei appendAudio")?;
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(audio_base64)
        .map_err(|e| format!("audioBase64 sollte gültiges Base64 sein: {e}"))?;
    if decoded != b"FAKE-MULAW-AUDIO-BYTES" {
        return Err(format!(
            "unerwarteter Audioinhalt bei appendAudio: {decoded:?}"
        ));
    }

    // Ein `talk.event` VOR der appendAudio-Bestätigung schicken - prüft,
    // dass `transcribe_via_gateway` Events tatsächlich sammelt, während es
    // noch auf die Antwort wartet, statt sie wie eine normale `res`-Antwort
    // stillschweigend zu verwerfen.
    let transcript_event = serde_json::json!({
        "type": "event",
        "event": "talk.event",
        "payload": {
            "transcriptionSessionId": session_id,
            "type": "transcript",
            "text": "Wie spät ist es?",
            "final": true,
        },
    });
    send(&mut ws, &transcript_event).await?;

    let append_ok = serde_json::json!({
        "type": "res",
        "id": append_req["id"],
        "ok": true,
        "payload": {"ok": true},
    });
    send(&mut ws, &append_ok).await?;

    let close_req = recv_json(&mut ws).await?;
    if close_req["method"] != "talk.session.close" {
        return Err(format!(
            "erwartet method=talk.session.close, bekam {close_req:?}"
        ));
    }
    if close_req["params"]["sessionId"] != session_id {
        return Err(format!(
            "falsche sessionId bei close: {:?}",
            close_req["params"]["sessionId"]
        ));
    }
    let close_ok = serde_json::json!({
        "type": "res",
        "id": close_req["id"],
        "ok": true,
        "payload": {"ok": true},
    });
    send(&mut ws, &close_ok).await?;

    let close_event = serde_json::json!({
        "type": "event",
        "event": "talk.event",
        "payload": {
            "transcriptionSessionId": session_id,
            "type": "close",
            "reason": "completed",
        },
    });
    send(&mut ws, &close_event).await?;

    let _ = ws.close(None).await;
    Ok(())
}

async fn handle_chat_send_connection(
    listener: &TcpListener,
    expected_target_channel: &str,
    final_response: &str,
) -> Result<(), String> {
    let mut ws = accept_and_handshake(listener).await?;

    let subscribe_req = recv_json(&mut ws).await?;
    if subscribe_req["method"] != "sessions.messages.subscribe" {
        return Err(format!(
            "erwartet method=sessions.messages.subscribe, bekam {subscribe_req:?}"
        ));
    }
    let subscribe_ok = serde_json::json!({
        "type": "res",
        "id": subscribe_req["id"],
        "ok": true,
        "payload": {"sessionKeys": [expected_target_channel]},
    });
    send(&mut ws, &subscribe_ok).await?;

    let chat_send_req = recv_json(&mut ws).await?;
    if chat_send_req["method"] != "chat.send" {
        return Err(format!(
            "erwartet method=chat.send, bekam {chat_send_req:?}"
        ));
    }
    let run_id = chat_send_req["params"]["idempotencyKey"]
        .as_str()
        .ok_or("idempotencyKey fehlte bei chat.send")?
        .to_string();

    let ack = serde_json::json!({
        "type": "res",
        "id": chat_send_req["id"],
        "ok": true,
        "payload": {"runId": run_id, "status": "started"},
    });
    send(&mut ws, &ack).await?;

    let delta = serde_json::json!({
        "type": "event",
        "event": "chat",
        "payload": {
            "runId": run_id,
            "sessionKey": expected_target_channel,
            "seq": 1,
            "state": "delta",
            "deltaText": final_response,
        },
    });
    send(&mut ws, &delta).await?;
    let final_event = serde_json::json!({
        "type": "event",
        "event": "chat",
        "payload": {
            "runId": run_id,
            "sessionKey": expected_target_channel,
            "seq": 2,
            "state": "final",
        },
    });
    send(&mut ws, &final_event).await?;

    let _ = ws.close(None).await;
    Ok(())
}

/// `tts.speak` ist anders als `talk.session.*`/`chat.send` ein einfacher
/// synchroner Request/Response-Aufruf ohne Session/Events - eine Antwort
/// genügt.
async fn handle_tts_speak_connection(
    listener: &TcpListener,
    expected_text: &str,
) -> Result<(), String> {
    let mut ws = accept_and_handshake(listener).await?;

    let speak_req = recv_json(&mut ws).await?;
    if speak_req["method"] != "tts.speak" {
        return Err(format!("erwartet method=tts.speak, bekam {speak_req:?}"));
    }
    if speak_req["params"]["text"] != expected_text {
        return Err(format!(
            "falscher Text bei tts.speak: {:?}",
            speak_req["params"]["text"]
        ));
    }

    use base64::Engine;
    let audio_base64 = base64::engine::general_purpose::STANDARD.encode(b"FAKE-TTS-AUDIO-BYTES");
    let speak_ok = serde_json::json!({
        "type": "res",
        "id": speak_req["id"],
        "ok": true,
        "payload": {
            "audioBase64": audio_base64,
            "provider": "test",
            "fileExtension": ".mp3",
        },
    });
    send(&mut ws, &speak_ok).await?;

    let _ = ws.close(None).await;
    Ok(())
}

async fn send(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    value: &serde_json::Value,
) -> Result<(), String> {
    use futures_util::SinkExt;
    ws.send(Message::Text(value.to_string()))
        .await
        .map_err(|e| format!("Mock-Gateway sollte senden können: {e}"))
}

async fn recv_json(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) -> Result<serde_json::Value, String> {
    use futures_util::StreamExt;
    let msg = ws
        .next()
        .await
        .ok_or("Verbindung sollte offen bleiben")?
        .map_err(|e| format!("gültiger WebSocket-Frame erwartet: {e}"))?;
    let Message::Text(text) = msg else {
        return Err(format!("erwartete Text-Frame, bekam {msg:?}"));
    };
    serde_json::from_str(&text).map_err(|e| format!("gültiges JSON erwartet: {e}"))
}
