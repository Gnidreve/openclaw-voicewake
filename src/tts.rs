use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::info;

use crate::config::TtsConfig;

/// Reine Argument-Konstruktion für piper, unabhängig testbar.
pub fn build_piper_args(cfg: &TtsConfig, out_wav: &Path) -> Vec<String> {
    let mut args = vec![
        "--output_file".to_string(),
        out_wav.to_string_lossy().to_string(),
    ];
    if let Some(model) = &cfg.model_path {
        args.push("--model".to_string());
        args.push(model.to_string_lossy().to_string());
    } else {
        args.push("--voice".to_string());
        args.push(cfg.voice.clone());
    }
    args.extend(cfg.extra_args.clone());
    args
}

/// Synthetisiert Text mit Piper und spielt das Ergebnis über das
/// macOS-Standardausgabegerät ab (via afplay, konfigurierbar).
pub async fn synthesize_and_play(cfg: &TtsConfig, text: &str, tmp_dir: &Path) -> Result<()> {
    let out_wav = tmp_dir.join(format!("piper-out-{}.wav", uuid::Uuid::new_v4()));
    let args = build_piper_args(cfg, &out_wav);

    info!(voice = %cfg.voice, "Starte Piper-TTS");

    let mut cmd = Command::new(&cfg.piper_binary);
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
    fn args_use_voice_when_no_model_path_set() {
        let cfg = TtsConfig::default();
        let args = build_piper_args(&cfg, &PathBuf::from("/tmp/out.wav"));
        assert!(args.contains(&"--voice".to_string()));
        assert!(args.contains(&"de_DE-thorsten-high".to_string()));
        assert!(!args.contains(&"--model".to_string()));
    }

    #[test]
    fn args_use_model_path_when_set() {
        let cfg = TtsConfig {
            model_path: Some(PathBuf::from("/models/thorsten.onnx")),
            ..Default::default()
        };
        let args = build_piper_args(&cfg, &PathBuf::from("/tmp/out.wav"));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"/models/thorsten.onnx".to_string()));
        assert!(!args.contains(&"--voice".to_string()));
    }
}
