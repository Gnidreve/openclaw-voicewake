use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Name des CoreAudio-Eingabegeräts. `None` = Systemstandard.
    pub device: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WakeWordConfig {
    /// Lokal laufendes, konfigurierbares Kommando zur Wake-Word-Erkennung.
    /// Muss bei Erkennung eine Zeile ausgeben, die `trigger_pattern` enthält.
    pub command: String,
    pub args: Vec<String>,
    pub trigger_pattern: String,
    pub restart_delay_ms: u64,
}
impl Default for WakeWordConfig {
    fn default() -> Self {
        Self {
            command: "openwakeword-listener".to_string(),
            args: vec![],
            trigger_pattern: "WAKE".to_string(),
            restart_delay_ms: 500,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct VadConfig {
    pub silence_timeout_ms: u64,
    pub max_recording_seconds: u64,
    pub silence_rms_threshold: f32,
    pub min_speech_ms: u64,
    pub frame_ms: u64,
}
impl Default for VadConfig {
    fn default() -> Self {
        Self {
            silence_timeout_ms: 4000,
            max_recording_seconds: 60,
            silence_rms_threshold: 0.02,
            min_speech_ms: 300,
            frame_ms: 30,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WhisperConfig {
    pub binary: String,
    pub model_path: PathBuf,
    pub language: String,
    pub extra_args: Vec<String>,
    pub timeout_secs: u64,
}
impl Default for WhisperConfig {
    fn default() -> Self {
        Self {
            binary: "whisper-cli".to_string(),
            model_path: PathBuf::from(
                "/Users/mac-mini/.openclaw/workspace/state/whisper.cpp-model-large-v3-turbo/ggml-large-v3-turbo.bin",
            ),
            language: "de".to_string(),
            extra_args: vec![],
            timeout_secs: 60,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OpenClawConfig {
    pub binary: String,
    /// Zielkanal/Agent. MUSS explizit gesetzt werden - kein automatischer
    /// Fallback auf die Main-Session.
    pub target_channel: String,
    pub extra_args: Vec<String>,
    pub timeout_secs: u64,
}
impl Default for OpenClawConfig {
    fn default() -> Self {
        Self {
            binary: "openclaw".to_string(),
            target_channel: String::new(),
            extra_args: vec![],
            timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TtsConfig {
    pub piper_binary: String,
    pub voice: String,
    pub model_path: Option<PathBuf>,
    pub extra_args: Vec<String>,
    pub player_binary: String,
    pub timeout_secs: u64,
}
impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            piper_binary: "piper".to_string(),
            voice: "de_DE-thorsten-high".to_string(),
            model_path: None,
            extra_args: vec![],
            player_binary: "afplay".to_string(),
            timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub ffmpeg_binary: String,
    pub temp_dir: Option<PathBuf>,
    pub log_level: String,
}
impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            ffmpeg_binary: "ffmpeg".to_string(),
            temp_dir: None,
            log_level: "info".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub audio: AudioConfig,
    pub wakeword: WakeWordConfig,
    pub vad: VadConfig,
    pub whisper: WhisperConfig,
    pub openclaw: OpenClawConfig,
    pub tts: TtsConfig,
    pub general: GeneralConfig,
}

impl Config {
    pub fn load(path: &std::path::Path) -> Result<Config> {
        if !path.exists() {
            anyhow::bail!(
                "Konfigurationsdatei nicht gefunden: {}. Kopiere config.example.toml nach config.toml und passe sie an.",
                path.display()
            );
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("Kann Konfigurationsdatei nicht lesen: {}", path.display()))?;
        let cfg: Config = toml::from_str(&raw).with_context(|| {
            format!("Kann Konfigurationsdatei nicht parsen: {}", path.display())
        })?;
        Ok(cfg)
    }

    /// Validiert die Konfiguration. Muss vor dem eigentlichen Betrieb aufgerufen werden.
    pub fn validate(&self, dry_run: bool) -> Result<()> {
        if self.openclaw.target_channel.trim().is_empty() {
            anyhow::bail!(
                "openclaw.target_channel ist nicht gesetzt. Der Zielkanal/Agent muss explizit \
                 konfiguriert werden und wird NICHT automatisch auf die Main-Session gesetzt."
            );
        }
        if !dry_run && !self.whisper.model_path.exists() {
            anyhow::bail!(
                "Whisper-Modell nicht gefunden: {}",
                self.whisper.model_path.display()
            );
        }
        Ok(())
    }
}

/// Lokaler Sprachdienst: Wake-Word -> VAD -> Whisper -> OpenClaw -> Piper TTS
#[derive(Debug, Parser)]
#[command(name = "claw-voice-bridge")]
pub struct Cli {
    /// Pfad zur TOML-Konfigurationsdatei
    #[arg(short, long, default_value = "config.toml")]
    pub config: PathBuf,

    /// Dry-Run: kein Mikrofon, kein echtes Wake-Word-Listening.
    #[arg(long)]
    pub dry_run: bool,

    /// Beispiel-WAV-Datei für den Dry-Run (ersetzt die Mikrofonaufnahme).
    #[arg(long)]
    pub dry_run_file: Option<PathBuf>,

    /// Überschreibt general.log_level aus der Konfigurationsdatei.
    #[arg(long)]
    pub log_level: Option<String>,

    /// Führt nur einen einzigen Zyklus aus und beendet sich danach.
    #[arg(long)]
    pub once: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let cfg = Config::default();
        assert_eq!(cfg.vad.silence_timeout_ms, 4000);
        assert_eq!(cfg.vad.max_recording_seconds, 60);
        assert_eq!(cfg.whisper.language, "de");
        assert_eq!(cfg.tts.voice, "de_DE-thorsten-high");
        assert!(cfg
            .whisper
            .model_path
            .to_string_lossy()
            .ends_with("ggml-large-v3-turbo.bin"));
    }

    #[test]
    fn validate_rejects_empty_target_channel() {
        let cfg = Config::default();
        assert!(cfg.validate(true).is_err());
    }

    #[test]
    fn validate_accepts_configured_channel_in_dry_run() {
        let mut cfg = Config::default();
        cfg.openclaw.target_channel = "voice-assistant".to_string();
        assert!(cfg.validate(true).is_ok());
    }

    #[test]
    fn cli_parses_dry_run_flags() {
        let cli = Cli::parse_from([
            "claw-voice-bridge",
            "--dry-run",
            "--dry-run-file",
            "sample.wav",
        ]);
        assert!(cli.dry_run);
        assert_eq!(cli.dry_run_file, Some(PathBuf::from("sample.wav")));
    }

    #[test]
    fn cli_defaults() {
        let cli = Cli::parse_from(["claw-voice-bridge"]);
        assert!(!cli.dry_run);
        assert_eq!(cli.config, PathBuf::from("config.toml"));
        assert!(!cli.once);
    }
}
