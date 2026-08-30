use anyhow::{Context, Result};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
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
        .stderr(Stdio::piped())
        // Falls diese Funktion (z. B. durch ein Shutdown-Signal) abgebrochen
        // wird, bevor der Wake-Word-Prozess selbst beendet ist, muss der
        // Prozess mit sterben statt verwaist weiterzulaufen.
        .kill_on_drop(true);

    info!(command = %cfg.command, "Starte Wake-Word-Erkennung");

    // ANDERS als bei ffmpeg/whisper-cli/OpenClaw-CLI/Piper bewusst OHNE
    // `child_process::spawn_isolated`: Der Wake-Word-Prozess ist der einzige
    // Aufruf, der tatsächlich das Mikrofon öffnet (das externe Skript startet
    // dafür intern sein eigenes ffmpeg). `spawn_isolated`s `process_group(0)`
    // hebt den Prozess in eine neue, von Terminal.app getrennte
    // Prozessgruppe - genau das bricht auf macOS die TCC-Vererbung der
    // Mikrofon-Berechtigung von Terminal.app auf den Kindprozess: Feldtest
    // nach 0.2.2 zeigte, dass der Prozess dann zwar startet, aber nie
    // Audio-Daten bekommt (ffmpeg hängt mit ~0% CPU in einem blockierenden
    // Read). Git-Historie bestätigt den Auslöser: `spawn_isolated` kam erst
    // mit 0.1.9 dazu, 0.1.6 (davor) funktionierte nachweislich noch. Der
    // Kompromiss: `kill_on_drop(true)` deckt weiterhin die direkte PID bei
    // Timeout/Shutdown ab: nur ein von diesem Skript selbst verwaistes
    // ffmpeg (falls es beim SIGKILL des Elternprozesses nicht selbst
    // aufräumt) wäre nicht erfasst - hinnehmbar gegenüber "Wake-Word
    // funktioniert gar nicht".
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

    // Muss parallel zum stdout-Zeilen-Lesen laufen, nicht erst danach: Läuft
    // der stderr-Puffer des Betriebssystems bei einem geschwätzigen Prozess
    // voll, würde dieser sonst blockieren, solange niemand ihn ausliest -
    // ein Deadlock. Wird nur bei einem Fehlschlag ausgewertet, siehe unten.
    let stderr = child
        .stderr
        .take()
        .context("Kein stderr vom Wake-Word-Prozess verfügbar")?;
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let mut stderr = stderr;
        let _ = stderr.read_to_string(&mut buf).await;
        buf
    });

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

    // `child.wait()` oben ist bereits durch: der Prozess ist beendet, sein
    // stderr-Pipe-Ende damit geschlossen - der Task liest sofort bis EOF
    // durch und blockiert hier nicht.
    let stderr_output = stderr_task.await.unwrap_or_default();

    // Nicht stillschweigend verwerfen: Ohne die stderr-Ausgabe des externen
    // Kommandos (fehlendes Modell, Python-Traceback, Mikrofon belegt, ...)
    // liest man aus "Wake-Word-Prozess unerwartet beendet" allein nichts
    // Brauchbares heraus.
    result.map_err(|e| {
        let stderr_output = stderr_output.trim();
        if stderr_output.is_empty() {
            e
        } else {
            e.context(format!("stderr: {stderr_output}"))
        }
    })
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

    /// Regression: stderr des Wake-Word-Prozesses ging bisher nach
    /// `Stdio::null()` komplett verloren - "Wake-Word-Prozess unerwartet
    /// beendet" allein sagt nichts darüber, WARUM (fehlendes Modell,
    /// Traceback, Mikrofon belegt, ...).
    #[tokio::test]
    async fn stderr_output_is_included_when_the_process_ends_without_a_trigger() {
        let cfg = WakeWordConfig {
            command: "/bin/sh".to_string(),
            args: ["-c", "echo 'kein Modell gefunden' >&2"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            trigger_pattern: "WAKE".to_string(),
            ..Default::default()
        };
        let err = wait_for_wakeword(&cfg).await.unwrap_err();
        assert!(err.to_string().contains("kein Modell gefunden"), "{err}");
    }
}
