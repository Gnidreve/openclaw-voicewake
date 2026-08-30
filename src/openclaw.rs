use anyhow::{bail, Context, Result};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

use crate::config::OpenClawConfig;
use crate::template::substitute;

/// Platzhalter für den Zielkanal bzw. Session-Key aus `target_channel`.
pub const CHANNEL_PLACEHOLDER: &str = "{channel}";
/// Platzhalter für die fertig gerenderte Nachricht.
pub const MESSAGE_PLACEHOLDER: &str = "{message}";
/// Platzhalter für das rohe Transkript innerhalb von `message_template`.
pub const TRANSCRIPT_PLACEHOLDER: &str = "{transcript}";

/// Setzt das Transkript in den konfigurierten Umschlag ein.
pub fn render_message(cfg: &OpenClawConfig, transcript: &str) -> String {
    cfg.message_template
        .replace(TRANSCRIPT_PLACEHOLDER, transcript)
}

/// Reine Argument-Konstruktion für das OpenClaw-CLI, unabhängig testbar.
/// Die Liste kommt vollständig aus der Konfiguration - nur so lässt sich
/// abbilden, dass der Zielkanal je nach CLI `--channel` oder `--session-key`
/// heißt und vor oder nach anderen Argumenten stehen muss.
pub fn build_openclaw_args(cfg: &OpenClawConfig, transcript: &str) -> Vec<String> {
    let message = render_message(cfg, transcript);
    let replacements = [
        (CHANNEL_PLACEHOLDER, cfg.target_channel.as_str()),
        (MESSAGE_PLACEHOLDER, message.as_str()),
    ];
    cfg.args
        .iter()
        .map(|arg| substitute(arg, &replacements))
        .collect()
}

/// Holt den Antworttext aus der Ausgabe des CLI.
///
/// Gibt das CLI JSON aus (`--json`), wird der Text aus der bekannten
/// Struktur gelesen: zuerst `result.payloads[0].text`, danach die
/// Ausweichfelder `reply`/`text`/`message`/`content`. Ist die Ausgabe kein
/// JSON-Objekt - etwa weil ein Adapter reinen Text liefert -, wird sie
/// unverändert (getrimmt) zurückgegeben. Damit funktioniert beides ohne
/// Konfigurationsschalter.
pub fn extract_response(stdout: &str) -> String {
    let trimmed = stdout.trim();

    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return trimmed.to_string();
    };
    let Some(object) = value.as_object() else {
        return trimmed.to_string();
    };

    if let Some(text) = object
        .get("result")
        .and_then(|r| r.get("payloads"))
        .and_then(|p| p.get(0))
        .and_then(|first| first.get("text"))
        .and_then(|t| t.as_str())
    {
        return text.trim().to_string();
    }

    for key in ["reply", "text", "message", "content"] {
        if let Some(text) = object.get(key).and_then(|v| v.as_str()) {
            return text.trim().to_string();
        }
    }

    // JSON, aber keine bekannte Form: lieber die Rohausgabe weiterreichen,
    // als still eine leere Antwort zu liefern - dann hört man, dass etwas
    // nicht stimmt, statt dass die Runde wortlos endet.
    warn!("JSON-Antwort ohne bekanntes Textfeld - gebe die Rohausgabe weiter");
    trimmed.to_string()
}

/// Übergibt den Transkript-Text über das konfigurierte CLI an OpenClaw und
/// liefert den Antworttext zurück.
pub async fn send_to_openclaw(cfg: &OpenClawConfig, transcript: &str) -> Result<String> {
    if cfg.target_channel.trim().is_empty() {
        bail!(
            "openclaw.target_channel ist leer - Abbruch, um kein unbeabsichtigtes \
             Fallback-Verhalten (z. B. Main-Session) zu riskieren"
        );
    }

    let args = build_openclaw_args(cfg, transcript);
    info!(channel = %cfg.target_channel, "Sende Transkript an OpenClaw");

    let mut cmd = Command::new(&cfg.binary);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = cmd.spawn().context("Kann OpenClaw-CLI nicht starten")?;
    let out = timeout(
        Duration::from_secs(cfg.timeout_secs),
        child.wait_with_output(),
    )
    .await
    .context("Timeout beim OpenClaw-Aufruf")??;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("OpenClaw-CLI fehlgeschlagen: {stderr}");
    }

    Ok(extract_response(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice_cfg() -> OpenClawConfig {
        OpenClawConfig {
            binary: "/opt/homebrew/bin/openclaw".to_string(),
            target_channel: "agent:main:voice-assistant".to_string(),
            args: [
                "agent",
                "--model",
                "deepseek/deepseek-v4-flash",
                "--session-key",
                "{channel}",
                "--message",
                "{message}",
                "--thinking",
                "low",
                "--json",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            message_template: "Zusatzregel: keine Emojis.\n\nTranskript:\n---\n{transcript}\n---"
                .to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn args_always_include_explicit_channel() {
        let cfg = OpenClawConfig {
            target_channel: "voice-assistant".to_string(),
            ..Default::default()
        };
        let args = build_openclaw_args(&cfg, "hallo welt");

        let channel_pos = args.iter().position(|a| a == "--channel").unwrap();
        assert_eq!(args[channel_pos + 1], "voice-assistant");

        let msg_pos = args.iter().position(|a| a == "--message").unwrap();
        assert_eq!(args[msg_pos + 1], "hallo welt");
    }

    /// Bildet den bisherigen openclaw-adapter.sh nach: eigener Session-Key
    /// statt `--channel`, feste Modell-/Thinking-Argumente, Umschlag um das
    /// Transkript. Schlägt dieser Test fehl, kann die Konfiguration das
    /// Skript nicht mehr ersetzen.
    #[test]
    fn reproduces_the_shell_adapter_invocation() {
        let args = build_openclaw_args(&voice_cfg(), "Wie spät ist es?");
        assert_eq!(
            args,
            vec![
                "agent",
                "--model",
                "deepseek/deepseek-v4-flash",
                "--session-key",
                "agent:main:voice-assistant",
                "--message",
                "Zusatzregel: keine Emojis.\n\nTranskript:\n---\nWie spät ist es?\n---",
                "--thinking",
                "low",
                "--json",
            ]
        );
    }

    /// Der Zielkanal steht jetzt wirklich in der Konfiguration: Ändert man
    /// ihn, ändert sich der Aufruf. Vorher legte das Adapter-Skript ihn fest
    /// und `target_channel` blieb wirkungslos.
    #[test]
    fn changing_the_target_channel_changes_the_call() {
        let mut cfg = voice_cfg();
        cfg.target_channel = "agent:main:andere-session".to_string();
        let args = build_openclaw_args(&cfg, "hi");
        assert!(args.contains(&"agent:main:andere-session".to_string()));
        assert!(!args.contains(&"agent:main:voice-assistant".to_string()));
    }

    /// Regression: `target_channel` konnte den literalen Text `{message}`
    /// enthalten (z. B. Zufall oder ein unglücklich gewählter Kanalname) und
    /// wurde dann von der nachfolgenden Message-Ersetzung fälschlich
    /// nochmal überschrieben, weil die alte Implementierung zwei verkettete
    /// `.replace()`-Aufrufe nacheinander ausführte.
    #[test]
    fn a_target_channel_containing_another_placeholders_text_is_not_corrupted() {
        let cfg = OpenClawConfig {
            target_channel: "chan-{message}-x".to_string(),
            args: ["--channel", "{channel}", "--message", "{message}"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ..Default::default()
        };
        let args = build_openclaw_args(&cfg, "hallo");
        let channel_pos = args.iter().position(|a| a == "--channel").unwrap();
        assert_eq!(args[channel_pos + 1], "chan-{message}-x");
    }

    #[test]
    fn message_template_wraps_the_transcript() {
        let cfg = voice_cfg();
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
    fn response_is_read_from_the_documented_json_shape() {
        let raw = r#"{"result":{"payloads":[{"text":"Es ist kurz nach acht."}]}}"#;
        assert_eq!(extract_response(raw), "Es ist kurz nach acht.");
    }

    #[test]
    fn response_falls_back_to_alternative_json_fields() {
        for (raw, expected) in [
            (r#"{"reply":"aus reply"}"#, "aus reply"),
            (r#"{"text":"aus text"}"#, "aus text"),
            (r#"{"message":"aus message"}"#, "aus message"),
            (r#"{"content":"aus content"}"#, "aus content"),
        ] {
            assert_eq!(extract_response(raw), expected, "{raw}");
        }
    }

    #[test]
    fn plain_text_output_is_passed_through() {
        assert_eq!(
            extract_response("  Einfach nur Text \n"),
            "Einfach nur Text"
        );
    }

    #[test]
    fn unknown_json_shape_falls_back_to_the_raw_output() {
        let raw = r#"{"unbekannt":123}"#;
        assert_eq!(extract_response(raw), raw);
    }

    #[test]
    fn json_that_is_not_an_object_is_passed_through() {
        // Eine Antwort wie "42" ist gültiges JSON, aber kein Objekt - sie
        // soll unverändert vorgelesen werden.
        assert_eq!(extract_response("42"), "42");
        assert_eq!(extract_response("[1,2,3]"), "[1,2,3]");
    }

    #[test]
    fn empty_payloads_fall_back_instead_of_panicking() {
        let raw = r#"{"result":{"payloads":[]},"reply":"Ausweichfeld"}"#;
        assert_eq!(extract_response(raw), "Ausweichfeld");
    }

    #[tokio::test]
    async fn empty_target_channel_is_rejected() {
        let cfg = OpenClawConfig {
            target_channel: "".into(),
            ..Default::default()
        };
        let result = send_to_openclaw(&cfg, "hallo").await;
        assert!(result.is_err());
    }
}
