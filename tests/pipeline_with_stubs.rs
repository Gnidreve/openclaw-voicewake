//! Integrationstest für den vollständigen Ablauf **ohne** real installierte
//! Werkzeuge.
//!
//! `tests/dry_run.rs` braucht echtes ffmpeg/whisper-cli/piper/openclaw und ist
//! deshalb `#[ignore]`. Dieser Test hier legt stattdessen Stub-Programme in
//! einem temporären Verzeichnis an, die sich wie die echten verhalten - und
//! die **hart fehlschlagen, wenn die Aufrufform nicht stimmt**. Damit läuft
//! die komplette Kette bei jedem `cargo test`, auch auf einer Maschine ohne
//! macOS, Mikrofon, Whisper-Modell oder OpenClaw.
//!
//! Der Test bildet bewusst die produktive Konfiguration nach:
//!   * Piper als Python-Modul in einem venv (`-m piper` muss zuerst kommen)
//!   * OpenClaw mit Subkommando `agent` und `--session-key` statt `--channel`
//!   * Antwort als JSON in der Form `result.payloads[0].text`
//!
//! Schlägt er fehl, kann die `config.toml` die früheren Shell-Adapter nicht
//! mehr ersetzen - genau das soll er verhindern.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Stub für das OpenClaw-CLI. Prüft die Aufrufform streng und antwortet in
/// der JSON-Struktur, die auch das echte CLI liefert.
const OPENCLAW_STUB: &str = r#"#!/bin/sh
set -eu
[ "$1" = "agent" ] || { echo "erwartet Subkommando 'agent', bekam: $1" >&2; exit 3; }
shift
session=""; msg=""; json=0
while [ $# -gt 0 ]; do
  case "$1" in
    --session-key) session="$2"; shift 2 ;;
    --message) msg="$2"; shift 2 ;;
    --json) json=1; shift ;;
    *) shift ;;
  esac
done
[ "$session" = "agent:main:voice-assistant" ] || { echo "falscher Session-Key: '$session'" >&2; exit 3; }
[ "$json" = "1" ] || { echo "--json fehlte" >&2; exit 3; }
case "$msg" in
  *"Zusatzregel"*) : ;;
  *) echo "Umschlag fehlt in der Nachricht" >&2; exit 3 ;;
esac
case "$msg" in
  *"Wie spaet ist es?"*) : ;;
  *) echo "Transkript fehlt in der Nachricht" >&2; exit 3 ;;
esac
printf '{"result":{"payloads":[{"text":"Es ist kurz nach acht."}]}}\n'
"#;

/// Stub für das Python eines venv: akzeptiert nur `-m piper` als erste
/// Argumente und erzeugt die WAV-Datei aus `-f`.
const VENV_PYTHON_STUB: &str = r#"#!/bin/sh
set -eu
[ "$1" = "-m" ] || { echo "erwartet -m als erstes Argument, bekam: $1" >&2; exit 3; }
[ "$2" = "piper" ] || { echo "erwartet Modul piper, bekam: $2" >&2; exit 3; }
shift 2
out=""; voice=""
while [ $# -gt 0 ]; do
  case "$1" in
    -m) voice="$2"; shift 2 ;;
    -f) out="$2"; shift 2 ;;
    *) shift ;;
  esac
done
[ -n "$out" ] || { echo "kein -f Ausgabepfad" >&2; exit 3; }
[ "$voice" = "de_DE-thorsten-high" ] || { echo "falsche Stimme: '$voice'" >&2; exit 3; }
cat > /dev/null
: > "$out"
"#;

/// Stub für ffmpeg: kopiert die Eingabe auf den Ausgabepfad.
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

/// Stub für whisper-cli: schreibt ein festes Transkript in <-of>.txt.
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

/// Stub für die Tonwiedergabe: tut nichts, meldet aber Erfolg.
const PLAYER_STUB: &str = "#!/bin/sh\nexit 0\n";

#[cfg(unix)]
#[test]
fn full_round_runs_against_stubbed_tools() {
    let dir = std::env::temp_dir().join(format!("voicebridge-pipeline-{}", uuid::Uuid::new_v4()));
    let bin_dir = dir.join("bin");
    std::fs::create_dir_all(&bin_dir).expect("Testverzeichnis anlegen");

    let openclaw = write_stub(&bin_dir, "openclaw-stub", OPENCLAW_STUB);
    let venv_python = write_stub(&bin_dir, "venv-python-stub", VENV_PYTHON_STUB);
    let ffmpeg = write_stub(&bin_dir, "ffmpeg-stub", FFMPEG_STUB);
    let whisper = write_stub(&bin_dir, "whisper-stub", WHISPER_STUB);
    let player = write_stub(&bin_dir, "player-stub", PLAYER_STUB);

    // whisper.model_path muss existieren, der Inhalt ist dem Stub egal.
    let model = dir.join("model.bin");
    std::fs::write(&model, b"").expect("Modell-Platzhalter schreiben");

    // Beispielaufnahme für den Dry-Run (Inhalt egal, der Stub liest sie nicht).
    let sample = dir.join("sample.wav");
    std::fs::write(&sample, b"RIFF").expect("Beispieldatei schreiben");

    let chat_log = dir.join("chat.log");
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!(
            r#"
[openclaw]
binary = "{openclaw}"
target_channel = "agent:main:voice-assistant"
args = ["agent", "--session-key", "{{channel}}", "--message", "{{message}}", "--json"]
message_template = """
Zusatzregel: keine Emojis.

Transkript:
---
{{transcript}}
---
"""

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
            openclaw = openclaw.display(),
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

    let output = Command::new(env!("CARGO_BIN_EXE_openclaw-voicebridge"))
        .args([
            "--config",
            config.to_str().unwrap(),
            "--dry-run",
            "--dry-run-file",
            sample.to_str().unwrap(),
            "--once",
        ])
        .output()
        .expect("Bridge starten");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let log = std::fs::read_to_string(&chat_log).unwrap_or_default();

    // Aufräumen vor den Assertions, damit auch ein Fehlschlag nichts liegen lässt.
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        log.contains(r#"[Input] "Wie spaet ist es?""#),
        "Transkript fehlt im Log.\nLog:\n{log}\nstderr:\n{stderr}"
    );
    assert!(
        log.contains(r#"[Output] "Es ist kurz nach acht.""#),
        "Antwort wurde nicht aus der JSON-Struktur gelesen.\nLog:\n{log}\nstderr:\n{stderr}"
    );
    // Die Stubs brechen mit Exit-Code 3 ab, wenn die Aufrufform nicht stimmt -
    // das schlüge oben schon fehl, hier zur Sicherheit noch einmal explizit.
    assert!(
        !stderr.contains("exit: 3") && !stderr.contains("erwartet"),
        "ein Stub hat die Aufrufform bemängelt:\n{stderr}"
    );
}
