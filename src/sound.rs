use anyhow::{bail, Context, Result};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::info;

use crate::config::SoundConfig;

/// Spielt den konfigurierten Bestätigungston ab (Standard: macOS-Systemsound
/// "Glass"), um Beginn/Ende der Aufnahme akustisch zu signalisieren. Ist
/// bewusst kein kritischer Schritt: Aufrufer sollten einen Fehler hier nur
/// loggen, nicht den laufenden Zyklus abbrechen.
pub async fn play_chime(cfg: &SoundConfig) -> Result<()> {
    if !cfg.enabled {
        return Ok(());
    }

    info!(path = %cfg.chime_path.display(), "Spiele Bestätigungston");

    let mut cmd = Command::new(&cfg.player_binary);
    cmd.arg(&cfg.chime_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let child = cmd
        .spawn()
        .context("Kann Wiedergabeprogramm für Bestätigungston nicht starten")?;
    let out = timeout(
        Duration::from_secs(cfg.timeout_secs),
        child.wait_with_output(),
    )
    .await
    .context("Timeout beim Abspielen des Bestätigungstons")??;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("Wiedergabe des Bestätigungstons fehlgeschlagen: {stderr}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_chime_does_not_spawn_a_process() {
        let cfg = SoundConfig {
            enabled: false,
            player_binary: "this-binary-does-not-exist-anywhere".to_string(),
            ..Default::default()
        };
        assert!(play_chime(&cfg).await.is_ok());
    }
}
