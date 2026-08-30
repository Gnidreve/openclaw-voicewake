//! Integrationstest für `--probe-gateway` (0.2.0) gegen ein selbstgebautes
//! Mock-Gateway statt einem echten OpenClaw-Prozess.
//!
//! Prüft den kompletten dokumentierten Handshake einmal End-to-End:
//! `connect.challenge` -> signierter `connect`-Request -> `hello-ok` ->
//! `sessions.messages.subscribe` -> Event. Die Ed25519-Signatur wird hier
//! serverseitig mit einer unabhängigen Implementierung (direkt über
//! `ed25519_dalek::Verifier`, nicht über den Code aus `device_identity.rs`)
//! nachgerechnet - sonst würde ein Fehler im Payload-Aufbau von Client UND
//! Prüfung gleichermaßen unbemerkt bleiben.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use std::process::Command;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

struct ExpectedV3Payload<'a> {
    device_id: &'a str,
    client_id: &'a str,
    client_mode: &'a str,
    role: &'a str,
    scopes: &'a [String],
    signed_at_ms: i64,
    token: &'a str,
    nonce: &'a str,
    platform: &'a str,
}

/// Rekonstruiert exakt `buildDeviceAuthPayloadV3` - unabhängig vom Code in
/// `src/device_identity.rs`, damit dieser Test einen Fehler dort auch
/// tatsächlich fangen kann.
fn expected_v3_payload(params: &ExpectedV3Payload<'_>) -> String {
    [
        "v3",
        params.device_id,
        params.client_id,
        params.client_mode,
        params.role,
        &params.scopes.join(","),
        &params.signed_at_ms.to_string(),
        params.token,
        params.nonce,
        params.platform,
        "",
    ]
    .join("|")
}

#[tokio::test]
async fn probe_gateway_completes_the_documented_handshake_against_a_mock_gateway() {
    let result = tokio::time::timeout(Duration::from_secs(20), run_test()).await;
    result.expect("Test ist hängen geblieben - vermutlich ein Deadlock im Handshake");
}

async fn run_test() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Mock-Gateway sollte einen Port belegen können");
    let port = listener.local_addr().unwrap().port();
    let expected_nonce = "test-nonce-12345";
    let expected_token = "shared-secret-for-test";
    let target_channel = "agent:main:voice-assistant";

    let server = tokio::spawn(run_mock_gateway(
        listener,
        expected_nonce.to_string(),
        expected_token.to_string(),
        target_channel.to_string(),
    ));

    let dir = std::env::temp_dir().join(format!("voicebridge-probe-{}", uuid::Uuid::new_v4()));
    let fake_home = dir.join("home");
    std::fs::create_dir_all(&fake_home).expect("Testverzeichnis anlegen");
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
gateway_token = "{expected_token}"
"#,
        ),
    )
    .expect("Konfiguration schreiben");

    let config_path = config.clone();
    let home_path = fake_home.clone();
    let client = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_openclaw-voicebridge"))
            .args(["--config", config_path.to_str().unwrap(), "--probe-gateway"])
            .env("HOME", &home_path)
            .output()
    });

    let output = client
        .await
        .expect("Task sollte nicht abbrechen")
        .expect("Bridge starten");
    server
        .await
        .expect("Mock-Gateway-Task sollte nicht abbrechen");

    // `tracing_subscriber::fmt()` schreibt standardmäßig nach stdout, nicht
    // stderr (siehe `init_logging` in `main.rs`).
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        stdout.contains("Gateway-Verbindung hergestellt"),
        "hello-ok wurde nicht wie erwartet verarbeitet.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("Zielkanal abonniert"),
        "sessions.messages.subscribe wurde nicht wie erwartet bestätigt.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("Gateway-Event") && stdout.contains("session.message"),
        "das vom Mock-Gateway gesendete Event wurde nicht protokolliert.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

async fn run_mock_gateway(
    listener: TcpListener,
    expected_nonce: String,
    expected_token: String,
    expected_target_channel: String,
) {
    let (stream, _) = listener
        .accept()
        .await
        .expect("Bridge sollte sich verbinden");
    let mut ws = tokio_tungstenite::accept_async(stream)
        .await
        .expect("WebSocket-Handshake sollte gelingen");

    // 1. Challenge senden.
    let challenge = serde_json::json!({
        "type": "event",
        "event": "connect.challenge",
        "payload": {"nonce": expected_nonce, "ts": 1_700_000_000_000i64},
    });
    send(&mut ws, &challenge).await;

    // 2. connect-Request empfangen und die Signatur unabhängig prüfen.
    let connect_req = recv_json(&mut ws).await;
    assert_eq!(connect_req["type"], "req");
    assert_eq!(connect_req["method"], "connect");
    let params = &connect_req["params"];
    assert_eq!(params["auth"]["token"], expected_token);
    assert_eq!(params["device"]["nonce"], expected_nonce);
    assert_eq!(params["role"], "operator");

    let scopes: Vec<String> = params["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let expected_payload = expected_v3_payload(&ExpectedV3Payload {
        device_id: params["device"]["id"].as_str().unwrap(),
        client_id: params["client"]["id"].as_str().unwrap(),
        client_mode: params["client"]["mode"].as_str().unwrap(),
        role: params["role"].as_str().unwrap(),
        scopes: &scopes,
        signed_at_ms: params["device"]["signedAt"].as_i64().unwrap(),
        token: &expected_token,
        nonce: &expected_nonce,
        platform: params["client"]["platform"].as_str().unwrap(),
    });
    let public_key_raw = URL_SAFE_NO_PAD
        .decode(params["device"]["publicKey"].as_str().unwrap())
        .expect("Public Key sollte gültiges Base64url sein");
    let verifying_key =
        VerifyingKey::from_bytes(&public_key_raw.try_into().unwrap()).expect("gültiger Public Key");
    let signature_raw = URL_SAFE_NO_PAD
        .decode(params["device"]["signature"].as_str().unwrap())
        .expect("Signatur sollte gültiges Base64url sein");
    let signature = Signature::from_bytes(&signature_raw.try_into().unwrap());
    verifying_key
        .verify(expected_payload.as_bytes(), &signature)
        .expect("Signatur sollte gegen den erwarteten v3-Payload verifizieren");

    // 3. hello-ok beantworten.
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
    send(&mut ws, &hello_ok).await;

    // 4. sessions.messages.subscribe empfangen und bestätigen.
    let subscribe_req = recv_json(&mut ws).await;
    assert_eq!(subscribe_req["method"], "sessions.messages.subscribe");
    assert_eq!(subscribe_req["params"]["key"], expected_target_channel);
    let subscribe_ok = serde_json::json!({
        "type": "res",
        "id": subscribe_req["id"],
        "ok": true,
        "payload": {"sessionKeys": [expected_target_channel]},
    });
    send(&mut ws, &subscribe_ok).await;

    // 5. Ein Event schicken, das laut Roadmap protokolliert werden soll.
    let event = serde_json::json!({
        "type": "event",
        "event": "session.message",
        "payload": {"text": "hallo"},
    });
    send(&mut ws, &event).await;

    // Verbindung schließen, damit sich die Bridge beendet, statt auf Strg+C
    // zu warten.
    let _ = ws.close(None).await;
}

async fn send(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    value: &serde_json::Value,
) {
    use futures_util::SinkExt;
    ws.send(Message::Text(value.to_string()))
        .await
        .expect("Mock-Gateway sollte senden können");
}

async fn recv_json(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) -> serde_json::Value {
    use futures_util::StreamExt;
    let msg = ws
        .next()
        .await
        .expect("Verbindung sollte offen bleiben")
        .expect("gültiger WebSocket-Frame");
    let Message::Text(text) = msg else {
        panic!("erwartete Text-Frame, bekam {msg:?}");
    };
    serde_json::from_str(&text).expect("gültiges JSON")
}
