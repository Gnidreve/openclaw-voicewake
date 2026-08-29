use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::info;

use crate::config::TtsConfig;

/// Platzhalter für den Pfad der zu erzeugenden WAV-Datei.
pub const OUTPUT_PLACEHOLDER: &str = "{output}";
/// Platzhalter für den Stimmennamen aus `tts.voice`.
pub const VOICE_PLACEHOLDER: &str = "{voice}";

/// Setzt die Platzhalter in `tts.args` ein. Die Argumentliste kommt
/// vollständig aus der Konfiguration - so lässt sich jede Piper-Variante
/// ansprechen (System-Binary, Python-Modul im venv, eigener Wrapper), ohne
/// dass die Bridge deren Flags kennen muss.
pub fn build_piper_args(cfg: &TtsConfig, out_wav: &Path) -> Vec<String> {
    let output = out_wav.to_string_lossy();
    cfg.args
        .iter()
        .map(|arg| {
            arg.replace(OUTPUT_PLACEHOLDER, &output)
                .replace(VOICE_PLACEHOLDER, &cfg.voice)
        })
        .collect()
}

/// Synthetisiert Text mit Piper und spielt das Ergebnis über das
/// macOS-Standardausgabegerät ab (via afplay, konfigurierbar).
pub async fn synthesize_and_play(cfg: &TtsConfig, text: &str, tmp_dir: &Path) -> Result<()> {
    let out_wav = tmp_dir.join(format!("piper-out-{}.wav", uuid::Uuid::new_v4()));
    let args = build_piper_args(cfg, &out_wav);

    info!(voice = %cfg.voice, "Starte Piper-TTS");

    let mut cmd = Command::new(&cfg.binary);
    cmd.args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().context("Kann Piper nicht starten")?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .context("Piper stdin nicht verfügbar")?;
        stdin.write_all(text.as_bytes()).await?;
    }

    let out = timeout(
        Duration::from_secs(cfg.timeout_secs),
        child.wait_with_output(),
    )
    .await
    .context("Timeout bei Piper-TTS")??;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("Piper fehlgeschlagen: {stderr}");
    }

    let play_result = play_wav(cfg, &out_wav).await;
    let _ = tokio::fs::remove_file(&out_wav).await;
    play_result
}

async fn play_wav(cfg: &TtsConfig, path: &Path) -> Result<()> {
    info!(?path, "Spiele Antwort über Standardausgabegerät ab");
    let mut cmd = Command::new(&cfg.player_binary);
    cmd.arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = cmd
        .spawn()
        .context("Kann Wiedergabeprogramm nicht starten")?;
    let out = timeout(
        Duration::from_secs(cfg.timeout_secs),
        child.wait_with_output(),
    )
    .await
    .context("Timeout bei der Wiedergabe")??;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("Wiedergabe fehlgeschlagen: {stderr}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn defaults_use_thorsten_voice_and_afplay() {
        let cfg = TtsConfig::default();
        assert_eq!(cfg.voice, "de_DE-thorsten-high");
        assert_eq!(cfg.player_binary, "afplay");
    }

    #[test]
    fn placeholders_are_substituted() {
        let cfg = TtsConfig::default();
        let args = build_piper_args(&cfg, &PathBuf::from("/tmp/out.wav"));
        assert_eq!(
            args,
            vec![
                "--model",
                "de_DE-thorsten-high",
                "--output_file",
                "/tmp/out.wav"
            ]
        );
    }

    /// Bildet den bisherigen piper-adapter.sh exakt nach - den Zweig, der im
    /// Feld tatsächlich lief (Stimmenname + --data-dir, nicht Modellpfad).
    /// Schlägt dieser Test fehl, kann die Konfiguration das Skript nicht
    /// mehr ersetzen.
    #[test]
    fn reproduces_the_venv_invocation_from_the_shell_adapter() {
        let cfg = TtsConfig {
            binary: "/Users/mac-mini/.openclaw/workspace/state/piper-tts-venv/bin/python3"
                .to_string(),
            voice: "de_DE-thorsten-high".to_string(),
            args: [
                "-m",
                "piper",
                "--data-dir",
                "/Users/mac-mini/.local/share/piper-voices",
                "-m",
                "{voice}",
                "-f",
                "{output}",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            ..Default::default()
        };
        let args = build_piper_args(&cfg, &PathBuf::from("/tmp/piper-out-42.wav"));
        assert_eq!(
            args,
            vec![
                "-m",
                "piper",
                "--data-dir",
                "/Users/mac-mini/.local/share/piper-voices",
                "-m",
                "de_DE-thorsten-high",
                "-f",
                "/tmp/piper-out-42.wav",
            ]
        );
    }

    #[test]
    fn placeholders_also_work_inside_a_combined_argument() {
        let cfg = TtsConfig {
            args: vec!["--output-file={output}".to_string()],
            ..Default::default()
        };
        let args = build_piper_args(&cfg, &PathBuf::from("/tmp/out.wav"));
        assert_eq!(args, vec!["--output-file=/tmp/out.wav"]);
    }

    #[test]
    fn arguments_without_placeholders_are_passed_through_unchanged() {
        let cfg = TtsConfig {
            args: vec!["--quiet".to_string(), "{output}".to_string()],
            ..Default::default()
        };
        let args = build_piper_args(&cfg, &PathBuf::from("/tmp/out.wav"));
        assert_eq!(args, vec!["--quiet", "/tmp/out.wav"]);
    }
}
