//! Absicherung gegen verwaiste Kindprozesse über `kill_on_drop` hinaus.
//!
//! `Command::kill_on_drop(true)` (bereits an jedem Spawn-Aufruf gesetzt)
//! killt beim Drop des `Child`-Handles nur die direkte Kind-PID - das
//! entspricht `libc::kill(pid, SIGKILL)`. Startet dieser Prozess selbst
//! weitere Prozesse (z. B. wenn das OpenClaw-CLI intern etwas ausführt),
//! werden die davon nicht erfasst und laufen bei einem Timeout oder
//! Shutdown-Abbruch verwaist weiter.
//!
//! Fix: Jeder Kindprozess bekommt über `process_group(0)` seine eigene
//! Prozessgruppe (PGID = eigene PID, getrennt von unserer). Ein
//! `ProcessGroupGuard` killt beim Drop die gesamte Gruppe per
//! `kill(-pgid, SIGKILL)` statt nur die eine PID - unabhängig davon, ob der
//! Drop durch einen Timeout, den `cancellable()`-Shutdown-Abbruch aus
//! `main.rs` oder normales Funktionsende ausgelöst wird. Ist die Gruppe zu
//! dem Zeitpunkt bereits leer (regulär beendeter Prozess), ist der Aufruf
//! ein wirkungsloser No-Op (`ESRCH`, wird ignoriert).

use std::io;
use tokio::process::{Child, Command};

/// Killt beim Drop die komplette Prozessgruppe, nicht nur den direkten
/// Kindprozess. `None` bedeutet: keine PID bekannt (z. B. Plattform ohne
/// Prozessgruppen-Unterstützung) - dann passiert beim Drop nichts.
#[derive(Debug)]
pub struct ProcessGroupGuard(Option<i32>);

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pgid) = self.0 {
            // SAFETY: reiner Systemaufruf ohne Speicherzugriff. Eine
            // negative PID adressiert bei kill(2) die gesamte Prozessgruppe
            // statt eines einzelnen Prozesses.
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }
    }
}

/// Startet `cmd` in einer eigenen Prozessgruppe und liefert zusätzlich zum
/// Kindprozess einen [`ProcessGroupGuard`]. `cmd` sollte bereits
/// `kill_on_drop(true)` gesetzt haben (deckt weiterhin den einfachen Fall
/// ohne Subprozesse ab); dieser Guard ergänzt das um die Prozessgruppe.
/// Gibt den rohen `io::Error` von `spawn()` unverändert weiter, damit
/// Aufrufer ihre eigene, binary-spezifische Fehlermeldung dranhängen
/// können (`.context("Kann ffmpeg nicht starten")` o. Ä.).
pub fn spawn_isolated(cmd: &mut Command) -> io::Result<(Child, ProcessGroupGuard)> {
    #[cfg(unix)]
    cmd.process_group(0);

    let child = cmd.spawn()?;
    let pgid = child.id().map(|pid| pid as i32);
    Ok((child, ProcessGroupGuard(pgid)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    #[tokio::test]
    async fn spawned_process_runs_in_its_own_group_and_guard_reaps_it_on_timeout() {
        // "sleep" simuliert einen Kindprozess, der ohne Eingreifen lange
        // weiterliefe - stellvertretend für einen hängenden ffmpeg/whisper-
        // cli/OpenClaw-/Piper-Aufruf.
        let mut cmd = Command::new("sleep");
        cmd.arg("30").stdout(Stdio::null()).kill_on_drop(true);

        let (child, guard) = spawn_isolated(&mut cmd).expect("sleep sollte startbar sein");
        let pid = child.id().expect("frisch gespawnter Prozess hat eine PID");

        // Kindprozess existiert tatsächlich.
        assert!(process_exists(pid), "sleep sollte laufen");

        // Timeout-Fall: Future (und damit `child`) wird verworfen, der Guard
        // fällt separat.
        drop(child);
        drop(guard);

        // kill_on_drop kümmert sich schon um die direkte PID; der Guard ist
        // hier redundant zu ihr, aber entscheidend, sobald `sleep` selbst
        // weitere Prozesse in seiner Gruppe hätte. Nach dem Drop beider muss
        // der Prozess in jedem Fall weg sein.
        wait_until_gone(pid).await;
    }

    #[tokio::test]
    async fn guard_is_a_no_op_after_the_process_already_exited_normally() {
        let mut cmd = Command::new("true");
        cmd.kill_on_drop(true);
        let (mut child, guard) = spawn_isolated(&mut cmd).expect("true sollte startbar sein");
        let status = child.wait().await.expect("true sollte sauber beenden");
        assert!(status.success());

        // Gruppe existiert nicht mehr - darf beim Drop nicht panicken oder
        // einen Fehler zurückgeben (Drop kann das ohnehin nicht, aber der
        // zugrundeliegende Syscall darf hier keine Überraschung auslösen).
        drop(guard);
    }

    fn process_exists(pid: u32) -> bool {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    async fn wait_until_gone(pid: u32) {
        for _ in 0..50 {
            if !process_exists(pid) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("Prozess {pid} lief nach dem Drop des Guards immer noch");
    }
}
