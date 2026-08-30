//! Integrationstest für `transport = "websocket"` (0.2.2, "Volle
//! Integration") gegen ein selbstgebautes Mock-Gateway statt einem echten
//! OpenClaw-Prozess - das WebSocket-Pendant zu
//! `tests/pipeline_with_stubs.rs` (CLI-Transport).
//!
//! Deckt den kompletten dokumentierten `chat.send`-Ablauf ab: Handshake
//! (wie in `tests/gateway_probe_with_mock_server.rs`) -> `chat.send` mit
//! `sessionKey`/`idempotencyKey` -> sofortiges ACK (`status: "started"`),
//! das laut Roadmap eine gesprochene Zwischenmeldung auslösen soll ->
//! gestreamte `deltaText`-Events -> `final` beendet die Runde. Piper wird
//! über einen Stub ersetzt, der den über stdin hereinkommenden Text
//! mitschreibt - so lässt sich nachweisen, dass tatsächlich zuerst die
//! Zwischenmeldung und danach die zusammengesetzte `deltaText`-Antwort
//! gesprochen wird, nicht nur, dass irgendein Text ankam.

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

const FFMPEG_STUB: &str = r#"#!/bin/sh
set -eu
input=""; out=""
while [ $# -gt 0 ]; do
  case "$1" in
    -i) input="$2"; shift 2 ;;
    *) out="$1"; shift ;;
  esac
done
cp "$input" "$out"
"#;

const WHISPER_STUB: &str = r#"#!/bin/sh
set -eu
of=""
while [ $# -gt 0 ]; do
  [ "$1" = "-of" ] && of="$2"
  shift
done
[ -n "$of" ] || { echo "-of fehlte" >&2; exit 3; }
printf 'Wie spaet ist es?\n' > "$of.txt"
"#;

const PLAYER_STUB: &str = "#!/bin/sh\nexit 0\n";

#[cfg(unix)]
#[tokio::test]
async fn websocket_round_streams_deltas_after_an_ack_triggered_interim_message() {
    let result = tokio::time::timeout(Duration::from_secs(20), run_test()).await;
    result.expect("Test ist hängen geblieben - vermutlich ein Deadlock im chat.send-Ablauf");
}

async fn run_test() {
    let dir =
        std::env::temp_dir().join(format!("voicebridge-ws-pipeline-{}", uuid::Uuid::new_v4()));
    let bin_dir = dir.join("bin");
    std::fs::create_dir_all(&bin_dir).expect("Testverzeichnis anlegen");

    let ffmpeg = write_stub(&bin_dir, "ffmpeg-stub", FFMPEG_STUB);
    let whisper = write_stub(&bin_dir, "whisper-stub", WHISPER_STUB);
    let player = write_stub(&bin_dir, "player-stub", PLAYER_STUB);

    // Piper-Stub, der den über stdin hereinkommenden Text mitschreibt - so
    // lässt sich die Reihenfolge (Zwischenmeldung vor der eigentlichen
    // Antwort) tatsächlich nachweisen statt nur zu raten.
    let spoken_log = dir.join("spoken.log");
    let venv_python = write_stub(
        &bin_dir,
        "venv-python-stub",
        &format!(
            r#"#!/bin/sh
set -eu
[ "$1" = "-m" ] || {{ echo "erwartet -m als erstes Argument, bekam: $1" >&2; exit 3; }}
[ "$2" = "piper" ] || {{ echo "erwartet Modul piper, bekam: $2" >&2; exit 3; }}
shift 2
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    -m) shift 2 ;;
    -f) out="$2"; shift 2 ;;
    *) shift ;;
  esac
done
[ -n "$out" ] || {{ echo "kein -f Ausgabepfad" >&2; exit 3; }}
cat >> "{spoken_log}"
printf '\n---\n' >> "{spoken_log}"
: > "$out"
"#,
            spoken_log = spoken_log.display(),
        ),
    );

    let model = dir.join("model.bin");
    std::fs::write(&model, b"").expect("Modell-Platzhalter schreiben");
    let sample = dir.join("sample.wav");
    std::fs::write(&sample, b"RIFF").expect("Beispieldatei schreiben");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Mock-Gateway sollte einen Port belegen können");
    let port = listener.local_addr().unwrap().port();
    let target_channel = "agent:main:voice-assistant";
    let expected_transcript = "Wie spaet ist es?";
    let interim_message = "Ich schaue nach.";

    let server = tokio::spawn(run_mock_gateway(
        listener,
        target_channel.to_string(),
        expected_transcript.to_string(),
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
gateway_host = "127.0.0.1"
gateway_port = {port}
interim_message = "{interim_message}"

[whisper]
binary = "{whisper}"
model_path = "{model}"

[tts]
binary = "{venv_python}"
voice = "de_DE-thorsten-high"
args = ["-m", "piper", "-m", "{{voice}}", "-f", "{{output}}"]
player_binary = "{player}"

[sound]
enabled = false

[general]
ffmpeg_binary = "{ffmpeg}"
temp_dir = "{tmp}"

[transcription_log]
path = "{chat_log}"
"#,
            whisper = whisper.display(),
            model = model.display(),
            venv_python = venv_python.display(),
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
        // Dieselbe Sperre wie in `tests/pipeline_with_stubs.rs`: der feste
        // Einzelinstanz-Sperrpfad ist nicht konfigurierbar, parallel
        // gestartete Bridge-Prozesse aus unterschiedlichen Testfiles würden
        // sich sonst gegenseitig die Sperre wegnehmen.
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
    let spoken = std::fs::read_to_string(&spoken_log).unwrap_or_default();

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        log.contains(r#"[Input] "Wie spaet ist es?""#),
        "Transkript fehlt im Log.\nLog:\n{log}\nstderr:\n{stderr}"
    );
    assert!(
        log.contains(r#"[Output] "Es ist kurz nach acht.""#),
        "aus deltaText zusammengesetzte Antwort fehlt im Log.\nLog:\n{log}\nstderr:\n{stderr}"
    );

    let spoken_segments: Vec<&str> = spoken
        .split("---")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(
        spoken_segments,
        vec![interim_message, "Es ist kurz nach acht."],
        "erwartet: erst die ACK-Zwischenmeldung, dann die aus deltaText zusammengesetzte Antwort - in dieser Reihenfolge.\nGesprochen:\n{spoken}\nstderr:\n{stderr}"
    );
}

/// Simuliert genau den Ausschnitt des Gateway-Protokolls, den
/// `send_chat_message` braucht: Handshake (leichtgewichtig, die volle
/// Signaturprüfung deckt bereits `tests/gateway_probe_with_mock_server.rs`
/// ab) -> Subscribe -> `chat.send` -> ACK -> gestreamte `chat`-Events.
async fn run_mock_gateway(
    listener: TcpListener,
    expected_target_channel: String,
    expected_transcript: String,
) -> Result<(), String> {
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

    let subscribe_req = recv_json(&mut ws).await?;
    if subscribe_req["method"] != "sessions.messages.subscribe" {
        return Err(format!(
            "erwartet method=sessions.messages.subscribe, bekam {subscribe_req:?}"
        ));
    }
    if subscribe_req["params"]["key"] != expected_target_channel.as_str() {
        return Err(format!(
            "falscher Zielkanal beim Subscribe: {:?}",
            subscribe_req["params"]["key"]
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
    // Regression: `chat.send` erwartet `sessionKey`, nicht `key` wie
    // `sessions.messages.subscribe`.
    if chat_send_req["params"]["sessionKey"] != expected_target_channel.as_str() {
        return Err(format!(
            "falscher sessionKey bei chat.send: {:?}",
            chat_send_req["params"]["sessionKey"]
        ));
    }
    if chat_send_req["params"]["message"] != expected_transcript.as_str() {
        return Err(format!(
            "falsche message bei chat.send: {:?}",
            chat_send_req["params"]["message"]
        ));
    }
    let run_id = chat_send_req["params"]["idempotencyKey"]
        .as_str()
        .ok_or("idempotencyKey fehlte bei chat.send")?
        .to_string();
    if run_id.is_empty() {
        return Err("idempotencyKey war leer".to_string());
    }

    let ack = serde_json::json!({
        "type": "res",
        "id": chat_send_req["id"],
        "ok": true,
        "payload": {"runId": run_id, "status": "started"},
    });
    send(&mut ws, &ack).await?;

    for (seq, delta_text) in [(1, "Es ist "), (2, "kurz nach acht.")] {
        let delta = serde_json::json!({
            "type": "event",
            "event": "chat",
            "payload": {
                "runId": run_id,
                "sessionKey": expected_target_channel,
                "seq": seq,
                "state": "delta",
                "deltaText": delta_text,
            },
        });
        send(&mut ws, &delta).await?;
    }
    let final_event = serde_json::json!({
        "type": "event",
        "event": "chat",
        "payload": {
            "runId": run_id,
            "sessionKey": expected_target_channel,
            "seq": 3,
            "state": "final",
        },
    });
    send(&mut ws, &final_event).await?;

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
