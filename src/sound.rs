use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::info;

use crate::child_process::spawn_isolated;
use crate::config::SoundConfig;

/// Spielt den konfigurierten Bestätigungston ab (Standard: macOS-Systemsound
/// "Glass"), um Beginn/Ende der Aufnahme akustisch zu signalisieren. Ist
/// bewusst kein kritischer Schritt: Aufrufer sollten einen Fehler hier nur
/// loggen, nicht den laufenden Zyklus abbrechen.
pub async fn play_chime(cfg: &SoundConfig) -> Result<()> {
    play(cfg, &cfg.chime_path, "Bestätigungston").await
}

/// Spielt den vom Bestätigungston unterscheidbaren Fehlerton ab (Standard:
/// macOS-Systemsound "Basso"). Damit ist hörbar, dass ein Zyklus abgebrochen
/// ist, statt dass das nur im Log steht. Ebenfalls unkritisch: Aufrufer
/// loggen einen Fehler hier nur.
pub async fn play_error_chime(cfg: &SoundConfig) -> Result<()> {
    play(cfg, &cfg.error_chime_path, "Fehlerton").await
}

async fn play(cfg: &SoundConfig, path: &Path, kind: &str) -> Result<()> {
    if !cfg.enabled {
        return Ok(());
    }

    info!(path = %path.display(), kind, "Spiele Signalton");

    let mut cmd = Command::new(&cfg.player_binary);
    cmd.arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let (child, _pg_guard) = spawn_isolated(&mut cmd)
        .with_context(|| format!("Kann Wiedergabeprogramm für {kind} nicht starten"))?;
    let out = timeout(
        Duration::from_secs(cfg.timeout_secs),
        child.wait_with_output(),
    )
    .await
    .with_context(|| format!("Timeout beim Abspielen von {kind}"))??;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("Wiedergabe von {kind} fehlgeschlagen: {stderr}");
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
        assert!(play_error_chime(&cfg).await.is_ok());
    }

    #[tokio::test]
    async fn missing_player_binary_is_reported_instead_of_panicking() {
        let cfg = SoundConfig {
            enabled: true,
            player_binary: "this-binary-does-not-exist-anywhere".to_string(),
            ..Default::default()
        };
        let err = play_error_chime(&cfg)
            .await
            .expect_err("fehlendes Wiedergabeprogramm muss einen Fehler liefern");
        assert!(err.to_string().contains("Fehlerton"), "{err}");
    }
}
