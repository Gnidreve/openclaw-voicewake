use anyhow::{Context, Result};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{info, warn};

use crate::config::WakeWordConfig;

/// Startet den konfigurierten externen Wake-Word-Prozess und wartet, bis eine
/// Zeile ausgegeben wird, die das konfigurierte Trigger-Muster enthält.
///
/// Der Prozess läuft vollständig lokal (kein Cloud-API-Aufruf) und wird nach
/// der Erkennung beendet. Dadurch "pausiert" die Wake-Word-Erkennung
/// automatisch während SPEAKING: diese Funktion wird nur im Zustand
/// LISTENING_FOR_WAKEWORD aufgerufen, sodass die eigene TTS-Ausgabe des
/// Systems keine erneute Erkennung auslösen kann.
pub async fn wait_for_wakeword(cfg: &WakeWordConfig) -> Result<()> {
    let mut cmd = Command::new(&cfg.command);
    cmd.args(&cfg.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Falls diese Funktion (z. B. durch ein Shutdown-Signal) abgebrochen
        // wird, bevor der Wake-Word-Prozess selbst beendet ist, muss der
        // Prozess mit sterben statt verwaist weiterzulaufen.
        .kill_on_drop(true);

    info!(command = %cfg.command, "Starte Wake-Word-Erkennung");

    let mut child = cmd.spawn().with_context(|| {
        format!(
            "Kann Wake-Word-Kommando nicht starten: '{}' (ist es installiert und im PATH?)",
            cfg.command
        )
    })?;

    let stdout = child
        .stdout
        .take()
        .context("Kein stdout vom Wake-Word-Prozess verfügbar")?;
    let mut lines = BufReader::new(stdout).lines();

    let result = loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.contains(&cfg.trigger_pattern) {
                    info!(%line, "Wake-Word erkannt");
                    break Ok(());
                }
            }
            Ok(None) => {
                warn!("Wake-Word-Prozess hat sich ohne Erkennung beendet");
                break Err(anyhow::anyhow!("Wake-Word-Prozess unerwartet beendet"));
            }
            Err(e) => break Err(e.into()),
        }
    };

    let _ = child.start_kill();
    let _ = child.wait().await;

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_trigger_pattern_is_wake() {
        let cfg = WakeWordConfig::default();
        assert_eq!(cfg.trigger_pattern, "WAKE");
        assert_eq!(cfg.restart_delay_ms, 500);
    }
}
