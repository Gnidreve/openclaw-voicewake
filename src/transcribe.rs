use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

use crate::config::{GeneralConfig, WhisperConfig};

/// Reine Argument-Konstruktion für ffmpeg, unabhängig testbar ohne Prozessstart.
pub fn build_ffmpeg_args(input: &Path, output: &Path) -> Vec<String> {
    vec![
        "-y".to_string(),
        "-i".to_string(),
        input.to_string_lossy().to_string(),
        "-ac".to_string(),
        "1".to_string(),
        "-ar".to_string(),
        "16000".to_string(),
        "-f".to_string(),
        "wav".to_string(),
        output.to_string_lossy().to_string(),
    ]
}

/// Normalisiert eine beliebige WAV-Datei via ffmpeg auf mono/16kHz PCM,
/// wie von whisper.cpp erwartet.
pub async fn normalize_audio(
    general: &GeneralConfig,
    input: &Path,
    output: &Path,
    timeout_secs: u64,
) -> Result<()> {
    let args = build_ffmpeg_args(input, output);
    info!(?input, ?output, "Normalisiere Audio mit ffmpeg");

    let mut cmd = Command::new(&general.ffmpeg_binary);
    cmd.args(&args).stdout(Stdio::null()).stderr(Stdio::piped());

    let child = cmd.spawn().context("Kann ffmpeg nicht starten")?;
    let out = timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
        .await
        .context("Timeout bei ffmpeg-Normalisierung")??;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("ffmpeg fehlgeschlagen: {stderr}");
    }
    Ok(())
}

/// Reine Argument-Konstruktion für whisper-cli, unabhängig testbar.
pub fn build_whisper_args(cfg: &WhisperConfig, wav_path: &Path, out_stem: &Path) -> Vec<String> {
    let mut args = vec![
        "-m".to_string(),
        cfg.model_path.to_string_lossy().to_string(),
        "-l".to_string(),
        cfg.language.clone(),
        "-f".to_string(),
        wav_path.to_string_lossy().to_string(),
        "-otxt".to_string(),
        "-of".to_string(),
        out_stem.to_string_lossy().to_string(),
    ];
    args.extend(cfg.extra_args.clone());
    args
}

/// Ruft whisper-cli auf und liefert den transkribierten Text zurück.
pub async fn transcribe(cfg: &WhisperConfig, wav_path: &Path, tmp_dir: &Path) -> Result<String> {
    if !cfg.model_path.exists() {
        bail!(
            "Whisper-Modell nicht gefunden: {}",
            cfg.model_path.display()
        );
    }

    let out_stem = tmp_dir.join(format!("whisper-out-{}", uuid::Uuid::new_v4()));
    let args = build_whisper_args(cfg, wav_path, &out_stem);

    info!(binary = %cfg.binary, "Starte Whisper-Transkription");

    let mut cmd = Command::new(&cfg.binary);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = cmd.spawn().context("Kann whisper-cli nicht starten")?;
    let out = timeout(
        Duration::from_secs(cfg.timeout_secs),
        child.wait_with_output(),
    )
    .await
    .context("Timeout bei Whisper-Transkription")??;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("whisper-cli fehlgeschlagen: {stderr}");
    }

    let txt_path: PathBuf = {
        let mut p = out_stem.clone();
        p.set_extension("txt");
        p
    };

    let text = tokio::fs::read_to_string(&txt_path)
        .await
        .with_context(|| format!("Kann Whisper-Ausgabe nicht lesen: {}", txt_path.display()))?;
    let _ = tokio::fs::remove_file(&txt_path).await;

    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        warn!("Whisper hat keinen Text erkannt");
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WhisperConfig;
    use std::path::PathBuf;

    #[test]
    fn whisper_config_defaults_use_expected_model_and_language() {
        let cfg = WhisperConfig::default();
        assert_eq!(cfg.language, "de");
        assert!(cfg.model_path.ends_with("ggml-large-v3-turbo.bin"));
    }

    #[test]
    fn whisper_args_contain_model_language_and_input() {
        let cfg = WhisperConfig::default();
        let wav = PathBuf::from("/tmp/in.wav");
        let out_stem = PathBuf::from("/tmp/out");
        let args = build_whisper_args(&cfg, &wav, &out_stem);

        assert_eq!(args[0], "-m");
        assert_eq!(args[1], cfg.model_path.to_string_lossy());
        assert_eq!(args[2], "-l");
        assert_eq!(args[3], "de");
        assert_eq!(args[4], "-f");
        assert_eq!(args[5], "/tmp/in.wav");
        assert!(args.contains(&"-otxt".to_string()));
    }

    #[test]
    fn whisper_args_append_extra_args() {
        let cfg = WhisperConfig {
            extra_args: vec!["--no-timestamps".to_string()],
            ..Default::default()
        };
        let args = build_whisper_args(&cfg, Path::new("in.wav"), Path::new("out"));
        assert_eq!(args.last(), Some(&"--no-timestamps".to_string()));
    }

    #[test]
    fn ffmpeg_args_normalize_to_mono_16k_wav() {
        let args = build_ffmpeg_args(Path::new("in.wav"), Path::new("out.wav"));
        assert!(args.contains(&"-ac".to_string()));
        assert!(args.contains(&"1".to_string()));
        assert!(args.contains(&"-ar".to_string()));
        assert!(args.contains(&"16000".to_string()));
        assert_eq!(args.last(), Some(&"out.wav".to_string()));
    }
}
