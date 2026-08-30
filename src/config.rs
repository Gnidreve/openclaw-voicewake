use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
#[serde(default, deny_unknown_fields)]
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
#[serde(default, deny_unknown_fields)]
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
    /// Untere Grenze für die Sprach-Erkennungsschwelle - wird nie
    /// unterschritten, auch wenn der nachgeführte Rauschboden (siehe
    /// `noise_floor_margin`) niedriger läge. Verhindert Übersensibilität in
    /// einem sehr leisen Raum (Mikrofon-Grundrauschen).
    pub silence_rms_threshold: f32,
    /// Zusätzlicher Abstand oberhalb des laufend nachgeführten
    /// Rauschbodens, den ein Frame überschreiten muss, um als Sprache zu
    /// gelten. Der Rauschboden folgt der tatsächlichen Umgebungslautstärke
    /// (z. B. ein laufender Fernseher) - dadurch bleibt die Sprach-Schwelle
    /// auch bei dauerhaftem Hintergrundgeräusch über dem Umgebungspegel,
    /// statt dass dieser permanent als Sprache gilt (siehe
    /// `transcript_filter` als nachgelagertes zweites Netz).
    pub noise_floor_margin: f32,
    /// Glättungsfaktor (0.0-1.0), mit dem der Rauschboden nach OBEN
    /// nachgeführt wird, wenn ein Frame lauter als der aktuelle Boden ist.
    /// Bewusst klein/langsam: Ein einzelner Sprechabschnitt (typisch wenige
    /// Sekunden) soll den Boden nicht selbst signifikant anheben, nur
    /// dauerhaftes Hintergrundgeräusch über viele Sekunden hinweg.
    pub noise_floor_rise_alpha: f32,
    /// Glättungsfaktor (0.0-1.0), mit dem der Rauschboden nach UNTEN
    /// nachgeführt wird, wenn ein Frame leiser als der aktuelle Boden ist.
    /// Bewusst groß/schnell: Sobald die Umgebung leiser wird (z. B. nach
    /// einem Sprechabschnitt), soll der Boden zügig wieder auf den echten
    /// Pegel zurückfallen, statt lange nachzuhängen.
    pub noise_floor_fall_alpha: f32,
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
            noise_floor_margin: 0.02,
            // ~30s bis zu ~95% Anpassung an einen neuen, dauerhaft
            // lauteren Pegel - deutlich langsamer als eine typische
            // Äußerung (wenige Sekunden), aber weit innerhalb von
            // `max_recording_seconds`.
            noise_floor_rise_alpha: 0.003,
            // ~1s bis zu ~95% Erholung, sobald die Umgebung wieder leiser
            // wird.
            noise_floor_fall_alpha: 0.1,
            min_speech_ms: 300,
            speech_gap_ms: 200,
            frame_ms: 30,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
#[serde(default, deny_unknown_fields)]
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
#[serde(default, deny_unknown_fields)]
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

/// Wie das Transkript an OpenClaw übergeben wird. `Cli` bleibt der
/// vollwertige Legacy-Pfad (siehe `openclaw.rs`), `Websocket` spricht direkt
/// mit dem Gateway (siehe `gateway_client.rs`) - beide werden dauerhaft
/// unterstützt, keiner ist ein reiner Fallback für den anderen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Cli,
    Websocket,
}

/// Wrapper um das Gateway-Token, damit das automatisch abgeleitete
/// `#[derive(Debug)]` von `OpenClawConfig` es nicht versehentlich in Logs
/// oder Fehlermeldungen ausgibt.
#[derive(Clone, Deserialize, Default)]
#[serde(transparent)]
pub struct GatewayToken(String);
impl GatewayToken {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
impl From<String> for GatewayToken {
    fn from(value: String) -> Self {
        Self(value)
    }
}
impl std::fmt::Debug for GatewayToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            write!(f, "GatewayToken(<leer>)")
        } else {
            write!(f, "GatewayToken(<redacted>)")
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OpenClawConfig {
    pub binary: String,
    /// Zielkanal/Agent bzw. Session-Key. MUSS explizit gesetzt werden - kein
    /// automatischer Fallback auf die Main-Session. Wird über `{channel}` in
    /// `args` eingesetzt.
    pub target_channel: String,
    /// Vollständige Argumentliste. Wie der Zielkanal übergeben wird, heißt je
    /// nach CLI anders (`--channel`, `--session-key`, ...), deshalb steht der
    /// Flag-Name hier und nicht im Code. Platzhalter:
    ///   `{channel}` - Wert aus `target_channel` (Pflicht)
    ///   `{message}` - die fertig gerenderte Nachricht (Pflicht)
    pub args: Vec<String>,
    /// Umschlag um das Transkript. `{transcript}` (Pflicht) wird durch den
    /// erkannten Text ersetzt. Gedacht für Formregeln wie "gut vorlesbare
    /// Sätze, keine Emojis" - solche Formulierungen sind Inhalt und gehören
    /// in die Konfiguration, nicht in kompilierten Code.
    pub message_template: String,
    pub timeout_secs: u64,
    /// Nach so vielen Sekunden ohne eine an OpenClaw gesendete Nachricht wird
    /// vor der nächsten Nachricht erst `session_reset_message` geschickt, um
    /// eine neue Session zu erzwingen - eine nach langer Pause fortgesetzte
    /// alte Session brächte sonst stark veralteten Kontext mit ein. `0`
    /// schaltet den Reset ab.
    pub session_reset_after_secs: u64,
    /// Nachricht, die für den Session-Reset an OpenClaw geschickt wird.
    pub session_reset_message: String,
    /// `"cli"` (Standard) oder `"websocket"`. Siehe `Transport`.
    pub transport: Transport,
    /// Hostname/IP des Gateways - nur relevant bei `transport = "websocket"`.
    /// Muss nicht `127.0.0.1` sein: Ein Gateway auf einem anderen Rechner im
    /// selben Tailnet/LAN oder über einen SSH-Tunnel ist ebenfalls gültig,
    /// solange die Gateway-eigene Authentifizierung das zulässt (siehe
    /// `gateway_token`).
    pub gateway_host: String,
    /// Port des Gateways - Standard `18789` (OpenClaws eigener Standardport).
    pub gateway_port: u16,
    /// Gemeinsames Gateway-Token (`gateway.auth.token` auf der
    /// OpenClaw-Seite). Leer = kein Token (nur zulässig, wenn das Gateway mit
    /// `gateway.auth.mode = "none"` läuft).
    pub gateway_token: GatewayToken,
    /// Nur relevant bei `transport = "websocket"`: wird direkt gesprochen,
    /// sobald `chat.send` sein ACK (`status: started`) liefert - das
    /// Pendant zum "Ich schau mir das an" aus Telegram, weil `chat.send`
    /// selbst non-blocking ist und die eigentliche Antwort erst über
    /// gestreamte `chat`-Events nachkommt (siehe `gateway_client.rs`).
    pub interim_message: String,
}
impl Default for OpenClawConfig {
    fn default() -> Self {
        Self {
            binary: "openclaw".to_string(),
            target_channel: String::new(),
            args: ["--channel", "{channel}", "--message", "{message}"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            // Standard: das Transkript unverändert weiterreichen. Wer eine
            // Formregel will, setzt sie bewusst in der eigenen Konfiguration.
            message_template: "{transcript}".to_string(),
            timeout_secs: 30,
            session_reset_after_secs: 3600,
            session_reset_message: "/new".to_string(),
            transport: Transport::Cli,
            gateway_host: "127.0.0.1".to_string(),
            gateway_port: 18789,
            gateway_token: GatewayToken::default(),
            interim_message: "Einen Moment, ich schaue nach.".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TtsConfig {
    /// Auszuführendes Programm. Muss nicht Piper selbst sein - bei einer
    /// venv-Installation steht hier das Python des venv, und `-m piper`
    /// kommt über `args`.
    pub binary: String,
    /// Stimmenname, einsetzbar in `args` über `{voice}`. Als eigenes Feld
    /// gehalten, damit ein Stimmenwechsel eine Zeile bleibt.
    pub voice: String,
    /// Vollständige Argumentliste - das ist bewusst kein "extra_args" mehr:
    /// Welche Flags Piper versteht, unterscheidet sich zwischen
    /// Installationen (venv-Modul, System-Binary, Wrapper), und das kann
    /// die Bridge nicht erraten. Platzhalter:
    ///   `{output}` - Pfad der zu erzeugenden WAV-Datei (Pflicht)
    ///   `{voice}`  - der Wert aus `voice`
    /// Der zu sprechende Text geht immer über stdin.
    pub args: Vec<String>,
    pub player_binary: String,
    pub timeout_secs: u64,
}
impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            binary: "piper".to_string(),
            voice: "de_DE-thorsten-high".to_string(),
            args: ["--model", "{voice}", "--output_file", "{output}"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            player_binary: "afplay".to_string(),
            timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
#[serde(default, deny_unknown_fields)]
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
#[serde(default, deny_unknown_fields)]
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
#[serde(default, deny_unknown_fields)]
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
        // Ohne `{channel}` erreicht der Zielkanal das CLI nicht - dann wäre
        // `target_channel` wirkungslos und die Nachricht liefe womöglich in
        // die Standard-Session. Genau das soll die Prüfung oben verhindern.
        if !self
            .openclaw
            .args
            .iter()
            .any(|arg| arg.contains(crate::openclaw::CHANNEL_PLACEHOLDER))
        {
            anyhow::bail!(
                "openclaw.args enthält keinen {} -Platzhalter. Der Zielkanal aus \
                 target_channel würde das CLI damit nie erreichen.",
                crate::openclaw::CHANNEL_PLACEHOLDER
            );
        }
        if !self
            .openclaw
            .args
            .iter()
            .any(|arg| arg.contains(crate::openclaw::MESSAGE_PLACEHOLDER))
        {
            anyhow::bail!(
                "openclaw.args enthält keinen {} -Platzhalter - das Transkript \
                 würde nie übergeben.",
                crate::openclaw::MESSAGE_PLACEHOLDER
            );
        }
        if !self
            .openclaw
            .message_template
            .contains(crate::openclaw::TRANSCRIPT_PLACEHOLDER)
        {
            anyhow::bail!(
                "openclaw.message_template enthält keinen {} -Platzhalter - der \
                 erkannte Text käme im Umschlag nicht vor.",
                crate::openclaw::TRANSCRIPT_PLACEHOLDER
            );
        }

        // Ohne diesen Platzhalter bekäme Piper nie einen Ausgabepfad: Es
        // entstünde keine WAV-Datei, und der Fehler zeigte sich erst beim
        // ersten Sprechversuch statt beim Start.
        if !self
            .tts
            .args
            .iter()
            .any(|arg| arg.contains(crate::tts::OUTPUT_PLACEHOLDER))
        {
            anyhow::bail!(
                "tts.args enthält keinen {} -Platzhalter. Ohne ihn weiß Piper nicht, \
                 wohin die Sprachausgabe geschrieben werden soll.",
                crate::tts::OUTPUT_PLACEHOLDER
            );
        }
        if !dry_run && !self.whisper.model_path.exists() {
            anyhow::bail!(
                "Whisper-Modell nicht gefunden: {}",
                self.whisper.model_path.display()
            );
        }

        // Ein leeres Muster passt per `str::contains` auf JEDE Zeile - die
        // Wake-Word-Erkennung würde dann beim ersten Prozess-Output sofort
        // auslösen, egal was ausgegeben wurde. Kein Absturz, aber eine
        // wirkungslose Wake-Word-Gate ohne jede Fehlermeldung.
        if self.wakeword.trigger_pattern.is_empty() {
            anyhow::bail!(
                "wakeword.trigger_pattern ist leer - das würde auf jede Ausgabezeile passen \
                 und die Wake-Word-Erkennung sofort auslösen, egal was ausgegeben wird."
            );
        }

        // `record_until_silence` rechnet `frame_ms` in die Frame-Größe um und
        // zählt bei jedem Frame `frame_ms` zu `elapsed_ms`/`silence_ms` dazu
        // (siehe `SilenceTracker::push_frame`). Bei 0 bliebe `elapsed_ms` für
        // immer bei 0 - weder `silence_timeout_ms` noch
        // `max_recording_seconds` könnten dann je ablaufen, die Aufnahme
        // liefe unbegrenzt weiter.
        if self.vad.frame_ms == 0 {
            anyhow::bail!(
                "vad.frame_ms ist 0 - silence_timeout_ms und max_recording_seconds könnten \
                 dann nie ablaufen, die Aufnahme liefe unbegrenzt weiter."
            );
        }

        // Der Reset-Mechanismus schickt `session_reset_message` unverändert
        // als eigenständige Nachricht (siehe `openclaw::send_raw_to_openclaw`)
        // - eine leere Nachricht wäre ein Aufruf ohne erkennbaren Zweck.
        if self.openclaw.session_reset_after_secs > 0
            && self.openclaw.session_reset_message.trim().is_empty()
        {
            anyhow::bail!(
                "openclaw.session_reset_message ist leer, obwohl \
                 session_reset_after_secs > 0 gesetzt ist - ohne Text hätte der \
                 Session-Reset keine erkennbare Wirkung."
            );
        }

        if self.openclaw.transport == Transport::Websocket {
            if self.openclaw.gateway_host.trim().is_empty() {
                anyhow::bail!(
                    "openclaw.gateway_host ist leer, obwohl transport = \"websocket\" gesetzt ist."
                );
            }
            if self.openclaw.gateway_port == 0 {
                anyhow::bail!(
                    "openclaw.gateway_port ist 0, obwohl transport = \"websocket\" gesetzt ist."
                );
            }
        }

        Ok(())
    }
}

/// Lokaler Sprachdienst: Wake-Word -> VAD -> Whisper -> OpenClaw -> Piper TTS
#[derive(Debug, Parser)]
#[command(name = "openclaw-voicebridge")]
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

    /// Diagnose-Werkzeug für `transport = "websocket"`: verbindet sich
    /// einmalig mit dem Gateway, abonniert `openclaw.target_channel` und
    /// protokolliert eingehende Events, ohne Mikrofon/Wake-Word/Piper
    /// anzufassen. Beendet sich mit Strg+C.
    #[arg(long)]
    pub probe_gateway: bool,
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

    /// Regression: Ohne `deny_unknown_fields` wurde ein unbekanntes Feld
    /// (Tippfehler oder ein Feldname aus einer alten Config-Version, z. B.
    /// `piper_binary`/`model_path`/`extra_args` aus vor-0.1.x-Configs)
    /// stillschweigend ignoriert - das betroffene Feld fiel dann unbemerkt
    /// auf seinen Default zurück statt dass das Laden fehlschlägt.
    #[test]
    fn unknown_field_is_rejected_instead_of_silently_falling_back_to_defaults() {
        let raw = r#"
            [tts]
            piper_binary = "/pfad/zur/venv-python"
            model_path = "/models/thorsten.onnx"
            extra_args = ["--foo"]
        "#;
        let err = toml::from_str::<Config>(raw).unwrap_err();
        assert!(
            err.to_string().contains("unknown field"),
            "unerwartete Fehlermeldung: {err}"
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
    fn validate_rejects_tts_args_without_output_placeholder() {
        let mut cfg = Config::default();
        cfg.openclaw.target_channel = "voice-assistant".to_string();
        cfg.tts.args = vec!["--model".to_string(), "{voice}".to_string()];
        let err = cfg.validate(true).unwrap_err();
        assert!(err.to_string().contains("{output}"), "{err}");
    }

    #[test]
    fn validate_accepts_configured_channel_in_dry_run() {
        let mut cfg = Config::default();
        cfg.openclaw.target_channel = "voice-assistant".to_string();
        assert!(cfg.validate(true).is_ok());
    }

    /// Regression: Ein leeres `trigger_pattern` passt per `str::contains`
    /// auf jede Ausgabezeile - die Wake-Word-Erkennung würde dann beim
    /// ersten Prozess-Output sofort und unbemerkt auslösen.
    #[test]
    fn validate_rejects_empty_trigger_pattern() {
        let mut cfg = Config::default();
        cfg.openclaw.target_channel = "voice-assistant".to_string();
        cfg.wakeword.trigger_pattern = String::new();
        let err = cfg.validate(true).unwrap_err();
        assert!(err.to_string().contains("trigger_pattern"), "{err}");
    }

    /// Regression: `frame_ms = 0` ließe `elapsed_ms` in
    /// `SilenceTracker::push_frame` für immer bei 0 stehen - weder
    /// `silence_timeout_ms` noch `max_recording_seconds` könnten dann je
    /// ablaufen.
    #[test]
    fn validate_rejects_zero_frame_ms() {
        let mut cfg = Config::default();
        cfg.openclaw.target_channel = "voice-assistant".to_string();
        cfg.vad.frame_ms = 0;
        let err = cfg.validate(true).unwrap_err();
        assert!(err.to_string().contains("frame_ms"), "{err}");
    }

    /// Regression: Eine leere `session_reset_message` bei aktiviertem Reset
    /// (`session_reset_after_secs > 0`) hätte keine erkennbare Wirkung.
    #[test]
    fn validate_rejects_empty_session_reset_message_when_reset_is_enabled() {
        let mut cfg = Config::default();
        cfg.openclaw.target_channel = "voice-assistant".to_string();
        cfg.openclaw.session_reset_after_secs = 3600;
        cfg.openclaw.session_reset_message = "  ".to_string();
        let err = cfg.validate(true).unwrap_err();
        assert!(err.to_string().contains("session_reset_message"), "{err}");
    }

    /// Eine leere `session_reset_message` ist unschädlich, solange der Reset
    /// über `session_reset_after_secs = 0` ohnehin abgeschaltet ist.
    #[test]
    fn validate_accepts_empty_session_reset_message_when_reset_is_disabled() {
        let mut cfg = Config::default();
        cfg.openclaw.target_channel = "voice-assistant".to_string();
        cfg.openclaw.session_reset_after_secs = 0;
        cfg.openclaw.session_reset_message = String::new();
        assert!(cfg.validate(true).is_ok());
    }

    #[test]
    fn transport_defaults_to_cli_with_localhost_gateway() {
        let cfg = OpenClawConfig::default();
        assert_eq!(cfg.transport, Transport::Cli);
        assert_eq!(cfg.gateway_host, "127.0.0.1");
        assert_eq!(cfg.gateway_port, 18789);
        assert!(cfg.gateway_token.is_empty());
    }

    #[test]
    fn interim_message_has_a_sensible_spoken_default() {
        let cfg = OpenClawConfig::default();
        assert_eq!(cfg.interim_message, "Einen Moment, ich schaue nach.");
    }

    #[test]
    fn websocket_transport_parses_a_custom_host_and_port() {
        let cfg: Config = toml::from_str(
            r#"
            [openclaw]
            target_channel = "voice-assistant"
            transport = "websocket"
            gateway_host = "100.64.1.5"
            gateway_port = 4711
            gateway_token = "shared-secret"
            "#,
        )
        .expect("Konfiguration sollte parsen");
        assert_eq!(cfg.openclaw.transport, Transport::Websocket);
        assert_eq!(cfg.openclaw.gateway_host, "100.64.1.5");
        assert_eq!(cfg.openclaw.gateway_port, 4711);
        assert_eq!(cfg.openclaw.gateway_token.as_str(), "shared-secret");
    }

    #[test]
    fn validate_rejects_empty_gateway_host_when_transport_is_websocket() {
        let mut cfg = Config::default();
        cfg.openclaw.target_channel = "voice-assistant".to_string();
        cfg.openclaw.transport = Transport::Websocket;
        cfg.openclaw.gateway_host = String::new();
        let err = cfg.validate(true).unwrap_err();
        assert!(err.to_string().contains("gateway_host"), "{err}");
    }

    #[test]
    fn validate_rejects_zero_gateway_port_when_transport_is_websocket() {
        let mut cfg = Config::default();
        cfg.openclaw.target_channel = "voice-assistant".to_string();
        cfg.openclaw.transport = Transport::Websocket;
        cfg.openclaw.gateway_port = 0;
        let err = cfg.validate(true).unwrap_err();
        assert!(err.to_string().contains("gateway_port"), "{err}");
    }

    #[test]
    fn validate_ignores_gateway_fields_when_transport_is_cli() {
        let mut cfg = Config::default();
        cfg.openclaw.target_channel = "voice-assistant".to_string();
        cfg.openclaw.gateway_host = String::new();
        cfg.openclaw.gateway_port = 0;
        assert!(cfg.validate(true).is_ok());
    }

    /// Ein Gateway-Token ist ein Geheimnis - `{:?}` darf es nie im Klartext
    /// preisgeben (z. B. wenn `Config` versehentlich in einer Fehlermeldung
    /// oder einem Debug-Log landet).
    #[test]
    fn gateway_token_debug_output_never_contains_the_token_value() {
        let token = GatewayToken("super-secret-value".to_string());
        let debug_output = format!("{token:?}");
        assert!(!debug_output.contains("super-secret-value"));
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
            "openclaw-voicebridge",
            "--dry-run",
            "--dry-run-file",
            "sample.wav",
        ]);
        assert!(cli.dry_run);
        assert_eq!(cli.dry_run_file, Some(PathBuf::from("sample.wav")));
    }

    #[test]
    fn cli_defaults() {
        let cli = Cli::parse_from(["openclaw-voicebridge"]);
        assert!(!cli.dry_run);
        assert_eq!(cli.config, PathBuf::from("config.toml"));
        assert!(!cli.once);
        assert!(!cli.probe_gateway);
    }

    #[test]
    fn cli_parses_probe_gateway_flag() {
        let cli = Cli::parse_from(["openclaw-voicebridge", "--probe-gateway"]);
        assert!(cli.probe_gateway);
    }
}
