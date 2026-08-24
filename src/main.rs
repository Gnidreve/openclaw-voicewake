mod audio;
mod config;
mod openclaw;
mod sound;
mod state;
mod transcribe;
mod tts;
mod vad;
mod wakeword;

use anyhow::{Context, Result};
use clap::Parser;
use config::{Cli, Config};
use state::{State, StateMachine};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

/// Wie oft der Shutdown-Flag während eines blockierenden Warteschritts
/// (z. B. Wake-Word-Erkennung) auf ein Signal geprüft wird.
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let cfg = Config::load(&cli.config).with_context(|| {
        format!(
            "Konfiguration konnte nicht geladen werden aus {:?}",
            cli.config
        )
    })?;

    init_logging(cli.log_level.as_deref().unwrap_or(&cfg.general.log_level));

    cfg.validate(cli.dry_run)?;

    if cli.dry_run && cli.dry_run_file.is_none() {
        warn!("Dry-Run ohne --dry-run-file: die Aufnahme kann nicht simuliert werden");
    }

    let shutdown = Arc::new(AtomicBool::new(false));
    spawn_signal_handler(shutdown.clone());

    info!(dry_run = cli.dry_run, "claw-voice-bridge gestartet");

    let mut sm = StateMachine::new();

    loop {
        if shutdown.load(Ordering::SeqCst) {
            info!("Beende auf Anfrage (Signal empfangen)");
            break;
        }

        if let Err(e) = run_cycle(&cfg, &cli, &mut sm, &shutdown).await {
            error!(error = %e, from = %sm.current(), "Fehler im Zyklus - kehre zu IDLE zurück");
            let _ = sm.transition(State::Idle);

            // Verhindert einen ungebremsten Busy-Loop, falls z. B. das
            // Wake-Word-Kommando dauerhaft fehlschlägt (fehlende Binary o. Ä.).
            if !cli.once && !shutdown.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(cfg.wakeword.restart_delay_ms)).await;
            }
        }

        if cli.once {
            info!("--once gesetzt, beende nach einem Zyklus");
            break;
        }
    }

    Ok(())
}

fn init_logging(level: &str) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

/// Pollt den Shutdown-Flag, bis er gesetzt ist. Dient dazu, blockierende
/// Warteschritte (z. B. die Wake-Word-Erkennung, die sonst unbegrenzt lang
/// laufen kann) per `tokio::select!` gegen ein SIGINT/SIGTERM abzubrechen,
/// damit der Dienst auch im Leerlauf sauber auf Ctrl+C reagiert.
async fn wait_for_shutdown(shutdown: &AtomicBool) {
    let mut interval = tokio::time::interval(SHUTDOWN_POLL_INTERVAL);
    loop {
        interval.tick().await;
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
    }
}

fn spawn_signal_handler(shutdown: Arc<AtomicBool>) {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = match signal(SignalKind::terminate()) {
                Ok(s) => s,
                Err(e) => {
                    error!(error = %e, "Kann SIGTERM-Handler nicht registrieren");
                    return;
                }
            };
            tokio::select! {
                _ = tokio::signal::ctrl_c() => info!("SIGINT empfangen"),
                _ = sigterm.recv() => info!("SIGTERM empfangen"),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        shutdown.store(true, Ordering::SeqCst);
    });
}

/// Führt genau einen vollständigen Zyklus der Zustandsmaschine aus:
/// IDLE -> LISTENING_FOR_WAKEWORD -> RECORDING -> TRANSCRIBING
///      -> SENDING_TO_OPENCLAW -> SPEAKING -> IDLE
async fn run_cycle(
    cfg: &Config,
    cli: &Cli,
    sm: &mut StateMachine,
    shutdown: &Arc<AtomicBool>,
) -> Result<()> {
    let tmp_dir = make_temp_dir(cfg)?;
    let result = run_cycle_inner(cfg, cli, sm, shutdown, &tmp_dir).await;
    cleanup_temp_dir(&tmp_dir);
    result
}

async fn run_cycle_inner(
    cfg: &Config,
    cli: &Cli,
    sm: &mut StateMachine,
    shutdown: &Arc<AtomicBool>,
    tmp_dir: &Path,
) -> Result<()> {
    sm.transition(State::ListeningForWakeword)?;
    if cli.dry_run {
        info!("[dry-run] Wake-Word wird simuliert erkannt");
    } else {
        tokio::select! {
            result = wakeword::wait_for_wakeword(&cfg.wakeword) => result?,
            _ = wait_for_shutdown(shutdown) => {
                sm.transition(State::Idle)?;
                return Ok(());
            }
        }
    }

    if shutdown.load(Ordering::SeqCst) {
        sm.transition(State::Idle)?;
        return Ok(());
    }

    // Solange eine Runde tatsächlich eine Antwort hervorbringt, bleibt der
    // Kanal für eine direkte Folgeeingabe offen - ohne dass das Wake-Word
    // erneut gesagt werden muss. Erst eine Runde ohne Sprache/Antwort
    // schließt den Kanal wieder (mit Ton als Signal, siehe unten).
    let mut is_followup_turn = false;

    loop {
        if shutdown.load(Ordering::SeqCst) {
            sm.transition(State::Idle)?;
            return Ok(());
        }

        sm.transition(State::Recording)?;
        let raw_wav = tmp_dir.join(format!("recording-{}.wav", uuid::Uuid::new_v4()));
        if cli.dry_run {
            let source = cli
                .dry_run_file
                .clone()
                .context("Im Dry-Run wird eine Beispieldatei benötigt (--dry-run-file)")?;
            tokio::fs::copy(&source, &raw_wav).await.with_context(|| {
                format!("Kann Beispieldatei nicht kopieren: {}", source.display())
            })?;
            info!(file = %source.display(), "[dry-run] verwende Beispieldatei als Aufnahme");
        } else {
            record_until_silence(cfg, &raw_wav).await?;
        }

        sm.transition(State::Transcribing)?;
        let normalized_wav = tmp_dir.join("normalized.wav");
        transcribe::normalize_audio(
            &cfg.general,
            &raw_wav,
            &normalized_wav,
            cfg.whisper.timeout_secs,
        )
        .await?;
        let transcript = transcribe::transcribe(&cfg.whisper, &normalized_wav, tmp_dir).await?;
        info!(%transcript, "Transkription abgeschlossen");

        sm.transition(State::SendingToOpenClaw)?;
        let response = if transcript.trim().is_empty() {
            warn!("Leeres Transkript - überspringe OpenClaw-Aufruf");
            String::new()
        } else {
            openclaw::send_to_openclaw(&cfg.openclaw, &transcript).await?
        };

        sm.transition(State::Speaking)?;
        if response.trim().is_empty() {
            info!("Keine Antwort von OpenClaw - keine Sprachausgabe");
            if is_followup_turn {
                // Kein Folge-Input erkannt - Kanal schließen und akustisch
                // markieren, dass ab jetzt wieder das Wake-Word nötig ist.
                if let Err(e) = sound::play_chime(&cfg.sound).await {
                    warn!(error = %e, "Konnte Kanal-geschlossen-Ton nicht abspielen");
                }
            }
            sm.transition(State::Idle)?;
            return Ok(());
        }

        // Wake-Word-Erkennung läuft nur im Zustand LISTENING_FOR_WAKEWORD und
        // wird hier bewusst nicht gestartet, damit die eigene Ausgabe keine
        // neue Wake-Word-Erkennung auslöst. Nach dem Vorlesen wird aber
        // direkt weiter aufgenommen (record_until_silence markiert Start/
        // Ende dieser Runde bereits per Ton), damit eine Folgeeingabe ohne
        // erneutes Wake-Word möglich ist.
        tts::synthesize_and_play(&cfg.tts, &response, tmp_dir).await?;

        if cli.dry_run {
            // Im Dry-Run würde dieselbe --dry-run-file bei erkannter
            // Sprache/Antwort sonst als Endlosschleife "weiterreden" -
            // ein Dry-Run-Durchlauf bleibt daher immer einmalig.
            sm.transition(State::Idle)?;
            return Ok(());
        }
        is_followup_turn = true;
    }
}

async fn record_until_silence(cfg: &Config, out_path: &Path) -> Result<()> {
    let mut capture = audio::start_capture(cfg.audio.device.as_deref())?;
    if let Err(e) = sound::play_chime(&cfg.sound).await {
        warn!(error = %e, "Konnte Aufnahme-Start-Ton nicht abspielen");
    }

    let mut tracker = vad::SilenceTracker::new(&cfg.vad);

    let frame_samples = (((capture.sample_rate as u64 * cfg.vad.frame_ms) / 1000) as usize
        * capture.channels as usize)
        .max(1);

    // Obergrenze vorab bekannt (max_recording_seconds) - Kapazität einmal
    // reservieren statt den Puffer währenddessen mehrfach wachsen und dabei
    // seinen kompletten Inhalt umkopieren zu lassen (bei 60s/48kHz/1ch sonst
    // bis zu mehrere MB unnötig kopierter Daten durch Reallocation).
    let expected_max_samples = (capture.sample_rate as u64
        * cfg.vad.max_recording_seconds
        * capture.channels as u64) as usize;
    let mut samples: Vec<f32> = Vec::with_capacity(expected_max_samples);
    let mut frame_buf: Vec<f32> = Vec::with_capacity(frame_samples * 2);

    'outer: loop {
        let chunk = match capture.receiver.recv().await {
            Some(c) => c,
            None => {
                warn!("Audio-Stream unerwartet beendet");
                break;
            }
        };
        samples.extend_from_slice(&chunk);
        frame_buf.extend_from_slice(&chunk);

        while frame_buf.len() >= frame_samples {
            // RMS direkt auf dem Slice berechnen statt den Frame per
            // drain().collect() zusätzlich in einen neuen Vec zu kopieren.
            let energy = vad::rms(&frame_buf[..frame_samples]);
            frame_buf.drain(..frame_samples);

            match tracker.push_frame(energy, cfg.vad.frame_ms) {
                vad::VadDecision::Continue => {}
                vad::VadDecision::StopSilence => {
                    info!("Stille erkannt - beende Aufnahme");
                    break 'outer;
                }
                vad::VadDecision::StopMaxDuration => {
                    info!("Maximale Aufnahmedauer erreicht - beende Aufnahme");
                    break 'outer;
                }
            }
        }
    }

    let dropped = capture.dropped_chunks.load(Ordering::Relaxed);
    if dropped > 0 {
        warn!(
            dropped_chunks = dropped,
            "Audio-Chunks wegen Backpressure verworfen - Aufnahme könnte kleine Lücken enthalten"
        );
    }
    audio::write_wav(out_path, &samples, capture.sample_rate, capture.channels)?;

    if let Err(e) = sound::play_chime(&cfg.sound).await {
        warn!(error = %e, "Konnte Aufnahme-Ende-Ton nicht abspielen");
    }

    Ok(())
}

fn make_temp_dir(cfg: &Config) -> Result<PathBuf> {
    let base = cfg
        .general
        .temp_dir
        .clone()
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("claw-voice-bridge-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).with_context(|| {
        format!(
            "Kann temporäres Verzeichnis nicht anlegen: {}",
            dir.display()
        )
    })?;
    Ok(dir)
}

fn cleanup_temp_dir(dir: &PathBuf) {
    if let Err(e) = std::fs::remove_dir_all(dir) {
        warn!(error = %e, dir = %dir.display(), "Konnte temporäres Verzeichnis nicht vollständig löschen");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wait_for_shutdown_resolves_once_flag_is_set() {
        let flag = AtomicBool::new(true);
        tokio::time::timeout(Duration::from_secs(1), wait_for_shutdown(&flag))
            .await
            .expect(
                "wait_for_shutdown sollte sofort zurückkehren, wenn der Flag bereits gesetzt ist",
            );
    }

    #[tokio::test]
    async fn wait_for_shutdown_does_not_resolve_while_unset() {
        let flag = AtomicBool::new(false);
        let result =
            tokio::time::timeout(Duration::from_millis(500), wait_for_shutdown(&flag)).await;
        assert!(
            result.is_err(),
            "wait_for_shutdown darf nicht zurückkehren, solange der Flag nicht gesetzt ist"
        );
    }
}
