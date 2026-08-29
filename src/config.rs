use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Name des CoreAudio-Eingabegeräts. `None` = Systemstandard.
    pub device: Option<String>,
    /// Wartezeit zwischen dem Start-Ton und dem Öffnen des Mikrofons.
    /// Lautsprecher und Raum klingen nach; ohne diese Pause landet das
    /// Ausklingen des Tons (bzw. das Ende einer gerade vorgelesenen Antwort)
    /// in der eigenen Aufnahme.
    pub mic_open_delay_ms: u64,
}
impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            device: None,
            mic_open_delay_ms: 200,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WakeWordConfig {
    /// Lokal laufendes, konfigurierbares Kommando zur Wake-Word-Erkennung.
    /// Muss bei Erkennung eine Zeile ausgeben, die `trigger_pattern` enthält.
    pub command: String,
    pub args: Vec<String>,
    pub trigger_pattern: String,
    /// Wartezeit vor einem erneuten Zyklusstart, nachdem ein Zyklus (i. d. R.
    /// wegen eines fehlgeschlagenen Wake-Word-Kommandos) mit Fehler
    /// abgebrochen wurde - verhindert einen ungebremsten Busy-Loop.
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
    /// Stille, nach der die Aufnahme endet - gilt ab dem ersten Frame, also
    /// auch dann, wenn nie jemand gesprochen hat. Sprechen setzt die Uhr
    /// zurück und verzögert das Ende dadurch.
    pub silence_timeout_ms: u64,
    /// Sicherheitsnetz gegen unbegrenztes Wachsen des Aufnahmepuffers, falls
    /// die Umgebung dauerhaft über `silence_rms_threshold` liegt und die
    /// Stille-Uhr deshalb nie abläuft. Bewusst hoch angesetzt: Es ist keine
    /// Längenbegrenzung für Sprache. `0` schaltet es ab.
    pub max_recording_seconds: u64,
    pub silence_rms_threshold: f32,
    pub min_speech_ms: u64,
    /// Zusammenhängende Stille, nach der ein laufender Sprach-Abschnitt als
    /// beendet gilt und `min_speech_ms` wieder von vorn zählt. Verhindert,
    /// dass sich verstreute laute Frames über die ganze Aufnahme zu
    /// "Sprache erkannt" aufaddieren; kurze Pausen zwischen Silben bleiben
    /// dabei unschädlich.
    pub speech_gap_ms: u64,
    pub frame_ms: u64,
}
impl Default for VadConfig {
    fn default() -> Self {
        Self {
            silence_timeout_ms: 4000,
            max_recording_seconds: 300,
            silence_rms_threshold: 0.02,
            min_speech_ms: 300,
            speech_gap_ms: 200,
            frame_ms: 30,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ConversationConfig {
    /// Wie viele Folgeeingaben nach einer vorgelesenen Antwort ohne erneutes
    /// Wake-Word möglich sind. Begrenzt den offenen Kanal, damit
    /// Fremdgeräusche (z. B. laufender Fernseher) ihn nicht endlos weiter
    /// offen halten können. `0` schaltet Folgeeingaben ganz ab - dann ist
    /// nach jeder Antwort wieder das Wake-Word nötig.
    pub max_followup_turns: u32,
}
impl Default for ConversationConfig {
    fn default() -> Self {
        Self {
            max_followup_turns: 3,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TranscriptFilterConfig {
    /// Transkripte, die eines dieser Muster enthalten, werden wie "keine
    /// Sprache" behandelt: kein OpenClaw-Aufruf, Kanal wird geschlossen.
    /// Gedacht für die typischen Whisper-Halluzinationen aus Hintergrund-/
    /// TV-Ton (Untertitel-Abspänne o. Ä.). Vergleich erfolgt normalisiert
    /// (Kleinschreibung, Satzzeichen ignoriert) als Teilstring-Suche.
    /// Leere Liste = Filter aus.
    pub ignored_patterns: Vec<String>,
}
impl Default for TranscriptFilterConfig {
    fn default() -> Self {
        Self {
            ignored_patterns: [
                "Untertitelung des ZDF",
                "Untertitel im Auftrag des ZDF",
                "Untertitelung im Auftrag des ZDF",
                "Untertitel von Stephanie Geiges",
                "Untertitelung aufgrund der Amara.org-Community",
                "Untertitel der Amara.org-Community",
                "Vielen Dank fürs Zuschauen",
                "Copyright WDR",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
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
pub struct SoundConfig {
    /// Bestätigungstöne bei Aufnahme-Start/-Ende an/aus.
    pub enabled: bool,
    /// Abzuspielende Sound-Datei (Standard: macOS-Systemsound "Glass").
    pub chime_path: PathBuf,
    /// Deutlich unterscheidbarer Ton für abgebrochene Zyklen (Standard:
    /// macOS-Systemsound "Basso") - damit ein Fehler nicht nur stumm im Log
    /// landet.
    pub error_chime_path: PathBuf,
    pub player_binary: String,
    pub timeout_secs: u64,
}
impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            chime_path: PathBuf::from("/System/Library/Sounds/Glass.aiff"),
            error_chime_path: PathBuf::from("/System/Library/Sounds/Basso.aiff"),
            player_binary: "afplay".to_string(),
            timeout_secs: 5,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TranscriptionLogConfig {
    /// Chat-artiges Log ("[Input] ..." / "[Output] ...") an/aus.
    pub enabled: bool,
    /// Wird bei jeder Zeile im Append-Modus geöffnet.
    pub path: PathBuf,
}
impl Default for TranscriptionLogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: PathBuf::from("transcription.log"),
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

impl GeneralConfig {
    /// Verzeichnis, unter dem die temporären Zyklusdaten liegen. Bewusst
    /// ohne Einfluss auf die Einzelinstanz-Sperre - deren Pfad ist fest,
    /// siehe `instance_lock::lock_path`.
    pub fn temp_base(&self) -> PathBuf {
        self.temp_dir.clone().unwrap_or_else(std::env::temp_dir)
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub audio: AudioConfig,
    pub wakeword: WakeWordConfig,
    pub vad: VadConfig,
    pub conversation: ConversationConfig,
    pub transcript_filter: TranscriptFilterConfig,
    pub whisper: WhisperConfig,
    pub openclaw: OpenClawConfig,
    pub tts: TtsConfig,
    pub sound: SoundConfig,
    pub transcription_log: TranscriptionLogConfig,
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
        // Sicherheitsnetz, keine Sprachlängenbegrenzung - deshalb bewusst
        // weit über jeder realistischen Äußerung.
        assert_eq!(cfg.vad.max_recording_seconds, 300);
        assert_eq!(cfg.whisper.language, "de");
        assert_eq!(cfg.tts.voice, "de_DE-thorsten-high");
        assert!(cfg
            .whisper
            .model_path
            .to_string_lossy()
            .ends_with("ggml-large-v3-turbo.bin"));
    }

    #[test]
    fn sound_defaults_use_glass_chime_and_afplay() {
        let cfg = Config::default();
        assert!(cfg.sound.enabled);
        assert_eq!(
            cfg.sound.chime_path,
            PathBuf::from("/System/Library/Sounds/Glass.aiff")
        );
        assert_eq!(cfg.sound.player_binary, "afplay");
    }

    #[test]
    fn error_chime_defaults_to_a_different_sound_than_the_confirmation_chime() {
        let cfg = Config::default();
        assert_eq!(
            cfg.sound.error_chime_path,
            PathBuf::from("/System/Library/Sounds/Basso.aiff")
        );
        assert_ne!(cfg.sound.error_chime_path, cfg.sound.chime_path);
    }

    #[test]
    fn conversation_defaults_bound_the_open_channel() {
        assert_eq!(Config::default().conversation.max_followup_turns, 3);
    }

    #[test]
    fn transcript_filter_defaults_cover_known_whisper_hallucinations() {
        let patterns = Config::default().transcript_filter.ignored_patterns;
        assert!(patterns.iter().any(|p| p.contains("ZDF")));
    }

    #[test]
    fn temp_base_follows_the_configured_temp_dir() {
        let cfg = GeneralConfig {
            temp_dir: Some(PathBuf::from("/tmp/voicewake")),
            ..Default::default()
        };
        assert_eq!(cfg.temp_base(), PathBuf::from("/tmp/voicewake"));
        assert_eq!(
            GeneralConfig::default().temp_base(),
            std::env::temp_dir(),
            "ohne temp_dir gilt das Systemtemp-Verzeichnis"
        );
    }

    #[test]
    fn explicit_sections_override_defaults_and_leave_the_rest_untouched() {
        let cfg: Config = toml::from_str(
            r#"
            [conversation]
            max_followup_turns = 7

            [transcript_filter]
            ignored_patterns = ["Nur dieses Muster"]
            "#,
        )
        .expect("Konfiguration sollte parsen");
        assert_eq!(cfg.conversation.max_followup_turns, 7);
        assert_eq!(
            cfg.transcript_filter.ignored_patterns,
            ["Nur dieses Muster"]
        );
        // Nicht gesetzte Abschnitte behalten ihre Defaults.
        assert_eq!(cfg.vad.silence_timeout_ms, 4000);
    }

    #[test]
    fn transcription_log_defaults_enabled_with_relative_path() {
        let cfg = Config::default();
        assert!(cfg.transcription_log.enabled);
        assert_eq!(
            cfg.transcription_log.path,
            PathBuf::from("transcription.log")
        );
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
    fn shipped_example_config_still_parses_and_validates() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.toml");
        let cfg = Config::load(&path).expect("config.example.toml sollte ladbar sein");
        cfg.validate(true)
            .expect("config.example.toml sollte die Validierung bestehen");
        // Stichproben aus den Abschnitten, die zuletzt dazugekommen sind -
        // damit ein neues Feld nicht nur im Code, sondern auch im Beispiel
        // landet.
        assert_eq!(cfg.conversation.max_followup_turns, 3);
        assert!(!cfg.transcript_filter.ignored_patterns.is_empty());
        assert_ne!(cfg.sound.error_chime_path, cfg.sound.chime_path);
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
