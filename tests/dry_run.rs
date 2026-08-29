//! Integrationstest für den kompletten Dry-Run-Ablauf mit einer
//! Beispiel-WAV-Datei (Deliverable: "Ein --dry-run-Test, der den kompletten
//! Ablauf mit einer Beispieldatei simuliert").
//!
//! Dieser Test startet die echte Binary im `--dry-run --once`-Modus und
//! lässt sie den vollen Pfad durchlaufen: Beispieldatei -> ffmpeg-Normalisierung
//! -> whisper-cli -> OpenClaw-CLI-Adapter -> Piper -> afplay.
//!
//! Er ist bewusst mit `#[ignore]` markiert, weil er reale, lokal installierte
//! Kommandos (`ffmpeg`, `whisper-cli` inkl. Modell, `piper`, `openclaw`,
//! `afplay`) auf dem PATH voraussetzt und sonst `cargo test` auf jeder
//! Maschine ohne diese Tools bricht. Ausführen mit:
//!
//!   cargo test --test dry_run -- --ignored --nocapture

use std::io::Write;
use std::process::Command;

/// Erzeugt eine winzige, gültige WAV-Beispieldatei (0.5s Stille, 16kHz mono)
/// als Ersatz für eine Mikrofonaufnahme.
fn write_sample_wav(path: &std::path::Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("Beispiel-WAV anlegen");
    for _ in 0..8_000 {
        writer.write_sample(0i16).unwrap();
    }
    writer.finalize().unwrap();
}

fn write_test_config(path: &std::path::Path) {
    let toml = r#"
[openclaw]
binary = "openclaw"
target_channel = "voice-assistant-test"
timeout_secs = 10

[whisper]
binary = "whisper-cli"
timeout_secs = 30

[tts]
piper_binary = "piper"
player_binary = "afplay"
timeout_secs = 10

[general]
log_level = "debug"
"#;
    let mut f = std::fs::File::create(path).expect("Test-Config anlegen");
    f.write_all(toml.as_bytes()).unwrap();
}

#[test]
#[ignore = "benötigt lokal installierte ffmpeg/whisper-cli/piper/openclaw/afplay Binaries"]
fn full_dry_run_cycle_completes_without_panicking() {
    let dir = tempfile_dir();
    let wav_path = dir.join("sample.wav");
    let config_path = dir.join("config.toml");
    write_sample_wav(&wav_path);
    write_test_config(&config_path);

    let bin = env!("CARGO_BIN_EXE_openclaw-voicebridge");
    let output = Command::new(bin)
        .arg("--config")
        .arg(&config_path)
        .arg("--dry-run")
        .arg("--dry-run-file")
        .arg(&wav_path)
        .arg("--once")
        .output()
        .expect("Binary konnte nicht gestartet werden");

    assert!(
        output.status.success(),
        "Dry-Run-Zyklus ist fehlgeschlagen.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn tempfile_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "openclaw-voicebridge-test-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
