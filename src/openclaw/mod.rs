//! Übergibt das Transkript an OpenClaw und liefert den Antworttext zurück.
//! Zwei Transporte, gesteuert über `openclaw.transport` (siehe
//! `config::Transport`), beide dauerhaft vollwertig unterstützt - keiner ist
//! ein Fallback des anderen:
//!   - [`cli`]: `openclaw agent --json` als Subprozess je Runde (Legacy-Pfad).
//!   - [`websocket`]: `chat.send` direkt gegen das Gateway.
//!
//! `render_message`/`reset_due` sind transportunabhängig (reine
//! Konfigurations-/Zeitlogik) und liegen deshalb hier statt in einem der
//! beiden Untermodule.

pub mod cli;
pub mod websocket;

use crate::config::OpenClawConfig;

/// Platzhalter für den Zielkanal bzw. Session-Key aus `target_channel`.
pub(crate) const CHANNEL_PLACEHOLDER: &str = "{channel}";
/// Platzhalter für die fertig gerenderte Nachricht.
pub(crate) const MESSAGE_PLACEHOLDER: &str = "{message}";
/// Platzhalter für das rohe Transkript innerhalb von `message_template`.
pub(crate) const TRANSCRIPT_PLACEHOLDER: &str = "{transcript}";

/// Setzt das Transkript in den konfigurierten Umschlag ein.
pub fn render_message(cfg: &OpenClawConfig, transcript: &str) -> String {
    cfg.message_template
        .replace(TRANSCRIPT_PLACEHOLDER, transcript)
}

/// Reine Entscheidung, ob vor der nächsten Nachricht erst ein Session-Reset
/// nötig ist - unabhängig testbar ohne echten Zeitablauf, da `Instant`s aus
/// der Vergangenheit direkt konstruierbar sind (`Instant::now() -
/// Duration::from_secs(x)`). Transportunabhängig: sowohl `cli` als auch
/// `websocket` fragen dieselbe Funktion.
pub fn reset_due(cfg: &OpenClawConfig, last_message_at: Option<std::time::Instant>) -> bool {
    if cfg.session_reset_after_secs == 0 {
        return false;
    }
    let Some(last) = last_message_at else {
        // Noch nie eine Nachricht gesendet - kein Reset nötig, es gibt noch
        // keine alte Session, die veralten könnte.
        return false;
    };
    last.elapsed() >= std::time::Duration::from_secs(cfg.session_reset_after_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn message_template_wraps_the_transcript() {
        let cfg = OpenClawConfig {
            message_template: "Zusatzregel: keine Emojis.\n\nTranskript:\n---\n{transcript}\n---"
                .to_string(),
            ..Default::default()
        };
        let message = render_message(&cfg, "Hallo");
        assert!(message.starts_with("Zusatzregel: keine Emojis."));
        assert!(message.contains("---\nHallo\n---"));
    }

    #[test]
    fn default_template_passes_the_transcript_through_unchanged() {
        let cfg = OpenClawConfig::default();
        assert_eq!(render_message(&cfg, "Hallo Welt"), "Hallo Welt");
    }

    #[test]
    fn reset_is_not_due_without_any_prior_message() {
        let cfg = OpenClawConfig::default();
        assert!(!reset_due(&cfg, None));
    }

    #[test]
    fn reset_is_disabled_when_session_reset_after_secs_is_zero() {
        let cfg = OpenClawConfig {
            session_reset_after_secs: 0,
            ..Default::default()
        };
        let long_ago = Instant::now() - Duration::from_secs(999_999);
        assert!(!reset_due(&cfg, Some(long_ago)));
    }

    #[test]
    fn reset_is_not_due_before_the_configured_threshold() {
        let cfg = OpenClawConfig {
            session_reset_after_secs: 3600,
            ..Default::default()
        };
        let recent = Instant::now() - Duration::from_secs(10);
        assert!(!reset_due(&cfg, Some(recent)));
    }

    #[test]
    fn reset_is_due_once_the_threshold_is_reached() {
        let cfg = OpenClawConfig {
            session_reset_after_secs: 3600,
            ..Default::default()
        };
        let long_ago = Instant::now() - Duration::from_secs(3601);
        assert!(reset_due(&cfg, Some(long_ago)));
    }
}
