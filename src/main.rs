mod audio;
mod config;
mod instance_lock;
mod openclaw;
mod sound;
mod state;
mod template;
mod transcribe;
mod transcript_filter;
mod transcript_log;
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

    // Muss vor allem anderen greifen: zwei gleichzeitig laufende Instanzen
    // starten je einen eigenen Wake-Word-Listener und greifen parallel auf
    // dasselbe Mikrofon zu. Parallelbetrieb ist nicht vorgesehen, die Sperre
    // deshalb ohne Ausnahme. Sie lebt bis zum Ende von `main`.
    let _instance_lock = instance_lock::InstanceLock::acquire(&instance_lock::lock_path())?;

    let shutdown = Arc::new(AtomicBool::new(false));
    spawn_signal_handler(shutdown.clone());

    info!(dry_run = cli.dry_run, "openclaw-voicebridge gestartet");

    let mut sm = StateMachine::new();

    loop {
        if shutdown.load(Ordering::SeqCst) {
            info!("Beende auf Anfrage (Signal empfangen)");
            break;
        }

        if let Err(e) = run_cycle(&cfg, &cli, &mut sm, &shutdown).await {
            error!(error = %e, from = %sm.current(), "Fehler im Zyklus - kehre zu IDLE zurück");

            // Fehlerton nur, wenn der Zyklus bereits über die Wake-Word-
            // Erkennung hinaus war: dort wartet jemand hörbar auf eine
            // Antwort. Ein dauerhaft fehlschlagendes Wake-Word-Kommando würde
            // sonst im Sekundentakt Fehlertöne erzeugen.
            if sm.current() != State::ListeningForWakeword {
                if let Err(se) = sound::play_error_chime(&cfg.sound).await {
                    warn!(error = %se, "Konnte Fehlerton nicht abspielen");
                }
            }

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
    // schließt den Kanal wieder, spätestens aber
    // `conversation.max_followup_turns` Folgerunden nach dem Wake-Word:
    // sonst können Fremdgeräusche im Raum (z. B. ein laufender Fernseher) den
    // Kanal beliebig lange offen halten.
    let mut followup_turns: u32 = 0;

    loop {
        if shutdown.load(Ordering::SeqCst) {
            sm.transition(State::Idle)?;
            return Ok(());
        }

        // Hier läuft für jede Runde exakt dieselbe Funktion - egal ob sie vom
        // Wake-Word oder von der vorigen Runde ausgelöst wurde. Der einzige
        // Unterschied zwischen "erste Runde" und "Folgerunde" ist der
        // Auslöser hier drumherum, nicht ihr Ablauf.
        if run_round(cfg, cli, sm, tmp_dir).await? == RoundOutcome::Closed {
            sm.transition(State::Idle)?;
            return Ok(());
        }

        if cli.dry_run {
            // Im Dry-Run würde dieselbe --dry-run-file bei erkannter
            // Sprache/Antwort sonst als Endlosschleife "weiterreden" -
            // ein Dry-Run-Durchlauf bleibt daher immer einmalig.
            sm.transition(State::Idle)?;
            return Ok(());
        }

        if followup_turns >= cfg.conversation.max_followup_turns {
            info!(
                max_followup_turns = cfg.conversation.max_followup_turns,
                "Maximale Zahl an Folgeeingaben erreicht - schließe den Kanal"
            );
            // Bewusst ohne eigenen Ton: Dass der Kanal zu ist, hört man
            // daran, dass nach der Antwort kein Start-Ton mehr kommt.
            sm.transition(State::Idle)?;
            return Ok(());
        }
        followup_turns += 1;
    }
}

/// Ergebnis einer Runde - entscheidet, ob der Kanal offen bleiben darf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoundOutcome {
    /// Eine Antwort wurde vorgelesen; eine Folgeeingabe ist möglich.
    Answered,
    /// Nichts erkannt oder keine Antwort - der Kanal wird geschlossen.
    Closed,
}

/// Eine vollständige Gesprächsrunde: Aufnahme -> Transkription -> OpenClaw
/// -> Sprachausgabe.
///
/// Bewusst *eine* Funktion für beide Auslöser (Wake-Word und Folgerunde).
/// Im Ablauf einer Runde gibt es keinen Unterschied zwischen der ersten und
/// jeder weiteren - und durch diesen Zuschnitt kann auch keiner entstehen.
/// Praktischer Nebeneffekt: Beim Debuggen taugt die erste Runde als Referenz
/// für alle folgenden.
async fn run_round(
    cfg: &Config,
    cli: &Cli,
    sm: &mut StateMachine,
    tmp_dir: &Path,
) -> Result<RoundOutcome> {
    sm.transition(State::Recording)?;
    let raw_wav = tmp_dir.join(format!("recording-{}.wav", uuid::Uuid::new_v4()));
    let speech_detected = if cli.dry_run {
        let source = cli
            .dry_run_file
            .clone()
            .context("Im Dry-Run wird eine Beispieldatei benötigt (--dry-run-file)")?;
        tokio::fs::copy(&source, &raw_wav)
            .await
            .with_context(|| format!("Kann Beispieldatei nicht kopieren: {}", source.display()))?;
        info!(file = %source.display(), "[dry-run] verwende Beispieldatei als Aufnahme");
        // Dry-Run testet bewusst den vollen Pfad inkl. Whisper, auch wenn
        // die Beispieldatei nur Stille enthält.
        true
    } else {
        record_until_silence(cfg, &raw_wav).await?
    };

    sm.transition(State::Transcribing)?;
    let transcript = if speech_detected {
        let normalized_wav = tmp_dir.join("normalized.wav");
        transcribe::normalize_audio(
            &cfg.general,
            &raw_wav,
            &normalized_wav,
            cfg.whisper.timeout_secs,
        )
        .await?;
        // Rohaufnahme wird nach der Normalisierung nicht mehr gebraucht -
        // unabhängig davon, was mit der Transkription/Antwort danach passiert.
        if let Err(e) = tokio::fs::remove_file(&raw_wav).await {
            warn!(error = %e, path = %raw_wav.display(), "Konnte Rohaufnahme nicht löschen");
        }

        let t = transcribe::transcribe(&cfg.whisper, &normalized_wav, tmp_dir).await?;

        // Normalisierte Aufnahme wird nach der Transkription nicht mehr
        // gebraucht - vor dem OpenClaw-Aufruf löschen statt erst am Zyklusende.
        if let Err(e) = tokio::fs::remove_file(&normalized_wav).await {
            warn!(error = %e, path = %normalized_wav.display(), "Konnte normalisierte Aufnahme nicht löschen");
        }
        t
    } else {
        // VAD hat während der gesamten Aufnahme keine Sprache erkannt
        // (nur Stille/Hintergrundrauschen) - Whisper wird erst gar nicht
        // aufgerufen, damit es aus nicht vorhandener Sprache nichts
        // heraushalluzinieren kann.
        info!("Keine Sprache erkannt - überspringe Normalisierung und Transkription");
        if let Err(e) = tokio::fs::remove_file(&raw_wav).await {
            warn!(error = %e, path = %raw_wav.display(), "Konnte Rohaufnahme nicht löschen");
        }
        String::new()
    };
    info!(%transcript, "Transkription abgeschlossen");

    // Whisper macht aus TV-/Hintergrundton gerne Abspann-Halluzinationen
    // ("Untertitelung des ZDF, ..."). Die zählen nicht als Eingabe -
    // sonst antwortet der Agent auf den Fernseher und hält den Kanal
    // dadurch offen.
    let transcript = match transcript_filter::matching_pattern(
        &cfg.transcript_filter.ignored_patterns,
        &transcript,
    ) {
        Some(pattern) => {
            warn!(
                %transcript,
                %pattern,
                "Transkript als Störgeräusch/Halluzination verworfen"
            );
            transcript_log::log_input_ignored(&cfg.transcription_log, &transcript).await;
            String::new()
        }
        None => {
            transcript_log::log_input(&cfg.transcription_log, &transcript).await;
            transcript
        }
    };

    sm.transition(State::SendingToOpenClaw)?;
    let response = if transcript.trim().is_empty() {
        warn!("Leeres Transkript - überspringe OpenClaw-Aufruf");
        String::new()
    } else {
        match openclaw::send_to_openclaw(&cfg.openclaw, &transcript).await {
            Ok(r) => r,
            Err(e) => {
                transcript_log::log_output(
                    &cfg.transcription_log,
                    transcript_log::OutputOutcome::Error,
                )
                .await;
                return Err(e);
            }
        }
    };

    sm.transition(State::Speaking)?;
    if response.trim().is_empty() {
        info!("Keine Antwort von OpenClaw - keine Sprachausgabe");
        transcript_log::log_output(
            &cfg.transcription_log,
            transcript_log::OutputOutcome::Skipped,
        )
        .await;
        // Bewusst ohne eigenen Ton: Wurde nichts erkannt, ist bereits der
        // Absende-Ton ausgeblieben - ein zusätzlicher, gleich klingender
        // Ton würde nur verwirren.
        return Ok(RoundOutcome::Closed);
    }

    // Wake-Word-Erkennung läuft nur im Zustand LISTENING_FOR_WAKEWORD und
    // wird hier bewusst nicht gestartet, damit die eigene Ausgabe keine
    // neue Wake-Word-Erkennung auslöst. Nach dem Vorlesen wird aber
    // direkt weiter aufgenommen (record_until_silence markiert Start/
    // Ende dieser Runde bereits per Ton), damit eine Folgeeingabe ohne
    // erneutes Wake-Word möglich ist.
    if let Err(e) = tts::synthesize_and_play(&cfg.tts, &response, tmp_dir).await {
        transcript_log::log_output(&cfg.transcription_log, transcript_log::OutputOutcome::Error)
            .await;
        return Err(e);
    }
    transcript_log::log_output(
        &cfg.transcription_log,
        transcript_log::OutputOutcome::Success(&response),
    )
    .await;

    Ok(RoundOutcome::Answered)
}

/// Nimmt bis Stille/Timeout auf und schreibt das Ergebnis nach `out_path`.
/// Gibt zurück, ob die VAD während der Aufnahme jemals echte Sprache
/// erkannt hat (siehe `SilenceTracker::speech_started`) - `false` bedeutet
/// reine Stille/Hintergrundrauschen, unabhängig vom rohen Audioinhalt.
async fn record_until_silence(cfg: &Config, out_path: &Path) -> Result<bool> {
    // Ton VOR dem Öffnen des Mikrofons. Bei einem Lautsprecher mit
    // integriertem Mikrofon (z. B. Anker PowerConf S330) landete er sonst in
    // der eigenen Aufnahme - laut und lang genug, um `speech_started` bei
    // *jeder* Aufnahme auszulösen. Damit lief Whisper auch über reine Stille
    // und halluzinierte daraus Text (Untertitel-Abspänne, "Vielen Dank."),
    // der als Eingabe den Kanal offen hielt.
    if let Err(e) = sound::play_chime(&cfg.sound).await {
        warn!(error = %e, "Konnte Aufnahme-Start-Ton nicht abspielen");
    }
    if cfg.audio.mic_open_delay_ms > 0 {
        // Lautsprecher und Raum klingen nach - das gilt auch für das Ende
        // einer gerade vorgelesenen Antwort in der Folgerunde.
        tokio::time::sleep(Duration::from_millis(cfg.audio.mic_open_delay_ms)).await;
    }

    let mut capture = audio::start_capture(cfg.audio.device.as_deref())?;

    let mut tracker = vad::SilenceTracker::new(&cfg.vad);

    let frame_samples = (((capture.sample_rate as u64 * cfg.vad.frame_ms) / 1000) as usize
        * capture.channels as usize)
        .max(1);

    // Kapazität für eine typische Äußerung vorab reservieren, damit der Puffer
    // im Normalfall nicht mehrfach wachsen und dabei seinen kompletten Inhalt
    // umkopieren muss. Bewusst *nicht* an `max_recording_seconds` gekoppelt:
    // das ist inzwischen ein hoch angesetztes Sicherheitsnetz (oder ganz aus),
    // danach würde hier bei jeder Aufnahme dreistellig MB reserviert. Längere
    // Aufnahmen lässt der Vec regulär wachsen.
    const PREALLOC_SECONDS: u64 = 15;
    let prealloc_samples =
        (capture.sample_rate as u64 * PREALLOC_SECONDS * capture.channels as u64) as usize;
    let mut samples: Vec<f32> = Vec::with_capacity(prealloc_samples);
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

    // Mikrofon schließen, bevor der Ende-Ton läuft - aus demselben Grund, aus
    // dem der Start-Ton vor dem Öffnen kommt: das Mikrofon soll nur offen
    // sein, solange wirklich aufgenommen wird.
    drop(capture);

    // Der zweite Ton bestätigt das Absenden, nicht bloß das Ende der
    // Aufnahme. Wurde keine Sprache erkannt, geht auch nichts an OpenClaw -
    // dann bleibt er aus, und genau dieses Ausbleiben ist das Signal
    // "nichts verstanden, nichts abgeschickt".
    let speech_detected = tracker.speech_started();
    if speech_detected {
        if let Err(e) = sound::play_chime(&cfg.sound).await {
            warn!(error = %e, "Konnte Absende-Ton nicht abspielen");
        }
    } else {
        info!("Keine Sprache erkannt - kein Absende-Ton, es wird nichts gesendet");
    }

    Ok(speech_detected)
}

fn make_temp_dir(cfg: &Config) -> Result<PathBuf> {
    let base = cfg.general.temp_base();
    let dir = base.join(format!("openclaw-voicebridge-{}", uuid::Uuid::new_v4()));
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
