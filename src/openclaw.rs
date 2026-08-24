use anyhow::{bail, Context, Result};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::info;

use crate::config::OpenClawConfig;

/// Reine Argument-Konstruktion für den OpenClaw-CLI-Adapter, unabhängig
/// testbar. Der Zielkanal/Agent wird immer explizit mitgegeben - es gibt
/// keinen Fallback, der stillschweigend die Main-Session befüllen könnte.
pub fn build_openclaw_args(cfg: &OpenClawConfig, transcript: &str) -> Vec<String> {
    let mut args = cfg.extra_args.clone();
    args.push("--channel".to_string());
    args.push(cfg.target_channel.clone());
    args.push("--message".to_string());
    args.push(transcript.to_string());
    args
}

/// Übergibt den Transkript-Text über den konfigurierten CLI-Adapter an
/// OpenClaw und liefert die Antwort (stdout) zurück.
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

    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn extra_args_are_prepended() {
        let cfg = OpenClawConfig {
            target_channel: "x".to_string(),
            extra_args: vec!["--profile".to_string(), "voice".to_string()],
            ..Default::default()
        };
        let args = build_openclaw_args(&cfg, "hi");
        assert_eq!(&args[0..2], &["--profile".to_string(), "voice".to_string()]);
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
