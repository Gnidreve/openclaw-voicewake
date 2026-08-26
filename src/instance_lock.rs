//! Einzelinstanz-Sperre.
//!
//! Hintergrund aus dem Feldtest: Liefen versehentlich zwei
//! claw-voice-bridge-Prozesse gleichzeitig, startete jeder von ihnen einen
//! eigenen Wake-Word-Listener. Beide griffen dann auf dasselbe Mikrofon zu und
//! nahmen parallel auf - was sich im Log als doppelte Listener-Starts pro
//! Zyklus zeigte und die Zyklen der beiden Instanzen ineinander laufen ließ.
//!
//! Die Sperre ist eine `flock`-Sperre auf einer Datei: Sie wird vom Kernel
//! gehalten, solange der Prozess lebt, und automatisch freigegeben, wenn er
//! endet - auch bei SIGKILL oder Absturz. Eine übrig gebliebene Sperrdatei ist
//! deshalb nie "stale": entscheidend ist die Sperre, nicht die Datei.

use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use tracing::{info, warn};

/// Hält die Sperre, solange dieser Wert lebt. Beim Drop (bzw. spätestens beim
/// Prozessende) wird sie freigegeben.
#[derive(Debug)]
pub struct InstanceLock {
    /// Nur zum Offenhalten des Dateideskriptors - die Sperre hängt daran.
    _file: File,
}

impl InstanceLock {
    /// Belegt die Sperre unter `path` oder schlägt mit einer Meldung fehl, die
    /// die PID der bereits laufenden Instanz nennt.
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Kann Verzeichnis für die Sperrdatei nicht anlegen: {}",
                    parent.display()
                )
            })?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("Kann Sperrdatei nicht öffnen: {}", path.display()))?;

        if !try_lock(&file)? {
            let holder = read_pid(&mut file);
            anyhow::bail!(
                "Es läuft bereits eine claw-voice-bridge-Instanz{} (Sperrdatei: {}). \
                 Zwei Instanzen würden sich um Mikrofon und Wake-Word-Listener streiten. \
                 Beende die laufende Instanz, oder setze general.single_instance = false, \
                 wenn das wirklich gewollt ist.",
                holder
                    .map(|pid| format!(" mit PID {pid}"))
                    .unwrap_or_default(),
                path.display()
            );
        }

        // PID nur zur Diagnose - die eigentliche Sperre ist das flock.
        let pid = std::process::id();
        if let Err(e) = write_pid(&mut file, pid) {
            warn!(error = %e, path = %path.display(), "Konnte PID nicht in die Sperrdatei schreiben");
        }

        info!(path = %path.display(), pid, "Einzelinstanz-Sperre belegt");
        Ok(Self { _file: file })
    }
}

fn write_pid(file: &mut File, pid: u32) -> std::io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(format!("{pid}\n").as_bytes())?;
    file.flush()
}

fn read_pid(file: &mut File) -> Option<u32> {
    let mut content = String::new();
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_to_string(&mut content).ok()?;
    content.trim().parse().ok()
}

/// `true` = Sperre belegt, `false` = bereits von einem anderen Prozess
/// gehalten. Ein `Err` bedeutet, dass das Sperren selbst fehlgeschlagen ist.
#[cfg(unix)]
fn try_lock(file: &File) -> Result<bool> {
    use std::os::unix::io::AsRawFd;

    // SAFETY: `file` ist offen und lebt länger als der Aufruf; der rohe fd ist
    // damit für die Dauer des flock-Aufrufs gültig.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::EWOULDBLOCK) => Ok(false),
        _ => Err(anyhow::Error::new(err).context("flock auf der Sperrdatei fehlgeschlagen")),
    }
}

#[cfg(not(unix))]
fn try_lock(_file: &File) -> Result<bool> {
    warn!("Einzelinstanz-Sperre wird auf dieser Plattform nicht unterstützt - übersprungen");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_lock_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "claw-voice-bridge-test-{}.lock",
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn acquire_writes_the_own_pid_into_the_lock_file() {
        let path = temp_lock_path();
        let lock = InstanceLock::acquire(&path).expect("erste Sperre sollte gelingen");

        let content = std::fs::read_to_string(&path).expect("Sperrdatei sollte lesbar sein");
        assert_eq!(content.trim(), std::process::id().to_string());

        drop(lock);
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn a_second_lock_on_the_same_path_is_rejected_and_released_again_on_drop() {
        let path = temp_lock_path();
        let first = InstanceLock::acquire(&path).expect("erste Sperre sollte gelingen");

        let err = InstanceLock::acquire(&path)
            .expect_err("zweite Sperre auf derselben Datei muss abgelehnt werden");
        let message = err.to_string();
        assert!(
            message.contains("bereits eine claw-voice-bridge-Instanz"),
            "unerwartete Meldung: {message}"
        );
        assert!(
            message.contains(&std::process::id().to_string()),
            "Meldung sollte die PID des Sperrhalters nennen: {message}"
        );

        drop(first);
        let again = InstanceLock::acquire(&path)
            .expect("nach Freigabe muss die Sperre wieder belegbar sein");
        drop(again);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn acquire_creates_missing_parent_directories() {
        let dir =
            std::env::temp_dir().join(format!("claw-voice-bridge-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("nested").join("bridge.lock");
        let lock = InstanceLock::acquire(&path).expect("Sperre sollte Verzeichnisse anlegen");
        assert!(path.exists());
        drop(lock);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
