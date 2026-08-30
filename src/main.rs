mod audio;
mod child_process;
mod config;
mod device_identity;
mod gateway_client;
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
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

/// Markiert einen Schritt, der wegen eines Shutdown-Signals abgebrochen
/// wurde - unterscheidbar von einem echten Fehler, damit dafür weder ein
/// Fehlerton spielt noch `error!` geloggt wird (siehe `main`).
#[derive(Debug, thiserror::Error)]
#[error("Abbruch angefordert (Shutdown)")]
struct ShutdownRequested;

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

    // `--probe-gateway` braucht wie `--dry-run` kein vorhandenes
    // Whisper-Modell - beides sind Diagnose-/Simulationswege, die nie
    // transkribieren.
    cfg.validate(cli.dry_run || cli.probe_gateway)?;

    // Reines Diagnose-Werkzeug: fasst weder Mikrofon noch Wake-Word-Listener
    // an, deshalb bewusst vor der Einzelinstanz-Sperre und ohne sie - ein
    // parallel laufender echter Zyklus wird dadurch nicht gestört.
    if cli.probe_gateway {
        return gateway_client::run_read_only_probe(&cfg).await;
    }

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
    // Persistiert über alle Zyklen hinweg, genau wie `sm` - bestimmt, wann
    // vor der nächsten OpenClaw-Nachricht ein Session-Reset fällig ist.
    let mut last_openclaw_message_at: Option<Instant> = None;

    loop {
        if shutdown.load(Ordering::SeqCst) {
            info!("Beende auf Anfrage (Signal empfangen)");
            break;
        }

        if let Err(e) = run_cycle(
            &cfg,
            &cli,
            &mut sm,
            &shutdown,
            &mut last_openclaw_message_at,
        )
        .await
        {
            if e.is::<ShutdownRequested>() {
                // Kein Fehler, sondern ein angeforderter Abbruch - weder
                // Fehlerton noch `error!`-Log dafür, die äußere Schleife
                // beendet sich im nächsten Durchlauf ohnehin über den
                // Shutdown-Flag.
                info!(from = %sm.current(), "Zyklus durch Shutdown abgebrochen");
            } else {
                error!(error = %e, from = %sm.current(), "Fehler im Zyklus - kehre zu IDLE zurück");

                // Fehlerton nur, wenn der Zyklus bereits über die Wake-Word-
                // Erkennung hinaus war: dort wartet jemand hörbar auf eine
                // Antwort. Ein dauerhaft fehlschlagendes Wake-Word-Kommando
                // würde sonst im Sekundentakt Fehlertöne erzeugen.
                if sm.current() != State::ListeningForWakeword {
                    if let Err(se) = sound::play_error_chime(&cfg.sound).await {
                        warn!(error = %se, "Konnte Fehlerton nicht abspielen");
                    }
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

/// Rennt `fut` gegen das Shutdown-Signal. Gewinnt das Signal, wird `fut`
/// verworfen - bei einem noch laufenden Kindprozess (ffmpeg, whisper-cli,
/// OpenClaw-CLI, Piper) sorgt dessen `kill_on_drop(true)` dafür, dass er
/// dabei beendet wird, statt verwaist weiterzulaufen - und `Err`
/// (`ShutdownRequested`) zurückgegeben, statt auf das natürliche Ende der
/// Phase zu warten. Deckt Aufnahme, Whisper, OpenClaw und TTS ab - die
/// Wake-Word-Wartephase hat ihren eigenen, schon bestehenden `select!`.
async fn cancellable<T>(shutdown: &AtomicBool, fut: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::select! {
        result = fut => result,
        _ = wait_for_shutdown(shutdown) => Err(ShutdownRequested.into()),
    }
}

/// Registriert Ctrl+C und (unter Unix) SIGTERM je in einer eigenen Task,
/// statt beide hinter einem gemeinsamen `select!` zu verstecken: Schlägt die
/// SIGTERM-Registrierung fehl (selten, aber möglich - z. B. in einer
/// eingeschränkten Sandbox), darf das Ctrl+C nicht mit lahmlegen. Beide Tasks
/// setzen unabhängig voneinander denselben `shutdown`-Flag.
fn spawn_signal_handler(shutdown: Arc<AtomicBool>) {
    tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            let _ = tokio::signal::ctrl_c().await;
            info!("SIGINT empfangen");
            shutdown.store(true, Ordering::SeqCst);
        }
    });

    #[cfg(unix)]
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                sigterm.recv().await;
                info!("SIGTERM empfangen");
                shutdown.store(true, Ordering::SeqCst);
            }
            Err(e) => {
                error!(
                    error = %e,
                    "Kann SIGTERM-Handler nicht registrieren - Ctrl+C bleibt trotzdem verfügbar"
                );
            }
        }
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
    last_openclaw_message_at: &mut Option<Instant>,
) -> Result<()> {
    let tmp_dir = make_temp_dir(cfg)?;
    let result = run_cycle_inner(cfg, cli, sm, shutdown, &tmp_dir, last_openclaw_message_at).await;
    cleanup_temp_dir(&tmp_dir);
    result
}

async fn run_cycle_inner(
    cfg: &Config,
    cli: &Cli,
    sm: &mut StateMachine,
    shutdown: &Arc<AtomicBool>,
    tmp_dir: &Path,
    last_openclaw_message_at: &mut Option<Instant>,
) -> Result<()> {
    sm.transition(State::ListeningForWakeword)?;
    if cli.dry_run {
        info!("[dry-run] Wake-Word wird simuliert erkannt");
    } else {
        // Echo-/Doppeltrigger-Schutz: Wake-Word-Lauschen darf nur in genau
        // diesem Zustand laufen, sonst könnte z. B. die eigene TTS-Ausgabe
        // erneut als Wake-Word eingelesen werden. Die `transition()` direkt
        // darüber sollte das schon garantieren - `require()` macht daraus
        // eine tatsächliche Bedingung statt nur eine Konvention.
        sm.require(State::ListeningForWakeword)?;
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
        if run_round(cfg, cli, sm, tmp_dir, shutdown, last_openclaw_message_at).await?
            == RoundOutcome::Closed
        {
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
    shutdown: &AtomicBool,
    last_openclaw_message_at: &mut Option<Instant>,
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
        cancellable(shutdown, record_until_silence(cfg, &raw_wav)).await?
    };

    sm.transition(State::Transcribing)?;
    let transcript = if speech_detected {
        let normalized_wav = tmp_dir.join("normalized.wav");
        cancellable(
            shutdown,
            transcribe::normalize_audio(
                &cfg.general,
                &raw_wav,
                &normalized_wav,
                cfg.whisper.timeout_secs,
            ),
        )
        .await?;
        // Rohaufnahme wird nach der Normalisierung nicht mehr gebraucht -
        // unabhängig davon, was mit der Transkription/Antwort danach passiert.
        if let Err(e) = tokio::fs::remove_file(&raw_wav).await {
            warn!(error = %e, path = %raw_wav.display(), "Konnte Rohaufnahme nicht löschen");
        }

        let t = cancellable(
            shutdown,
            transcribe::transcribe(&cfg.whisper, &normalized_wav, tmp_dir),
        )
        .await?;

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
    // Unterscheidet die beiden Wege in den "leere Antwort"-Zweig unten:
    // gar nicht erst gesendet (nichts erkannt) vs. gesendet, aber ohne
    // Antworttext zurückbekommen.
    let sent_to_openclaw = !transcript.trim().is_empty();
    let response = if !sent_to_openclaw {
        warn!("Leeres Transkript - überspringe OpenClaw-Aufruf");
        String::new()
    } else {
        // Nicht kritisch für die Runde: Schlägt der Reset fehl, wird trotzdem
        // ganz normal mit der eigentlichen Nachricht weitergemacht - ein
        // Reset-Fehlschlag soll keine sonst funktionierende Runde abbrechen.
        if let Err(e) = cancellable(
            shutdown,
            openclaw::maybe_reset_session(&cfg.openclaw, *last_openclaw_message_at),
        )
        .await
        {
            warn!(error = %e, "Session-Reset fehlgeschlagen - fahre trotzdem fort");
        }

        match cancellable(
            shutdown,
            openclaw::send_to_openclaw(&cfg.openclaw, &transcript),
        )
        .await
        {
            Ok(r) => {
                *last_openclaw_message_at = Some(Instant::now());
                r
            }
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
        if sent_to_openclaw {
            // Wurde etwas abgeschickt, aber keine Antwort erhalten, bliebe
            // das sonst akustisch unbemerkt - anders als bei "nichts
            // erkannt" (kein Ton, siehe unten) ist hier tatsächlich etwas
            // fehlgeschlagen, auch ohne dass `send_to_openclaw` selbst einen
            // Fehler zurückgab.
            if let Err(e) = sound::play_error_chime(&cfg.sound).await {
                warn!(error = %e, "Konnte Fehlerton nicht abspielen");
            }
        }
        // Wurde nichts erkannt, ist bereits der Absende-Ton ausgeblieben -
        // ein zusätzlicher, gleich klingender Ton würde nur verwirren.
        return Ok(RoundOutcome::Closed);
    }

    // Wake-Word-Erkennung läuft nur im Zustand LISTENING_FOR_WAKEWORD und
    // wird hier bewusst nicht gestartet, damit die eigene Ausgabe keine
    // neue Wake-Word-Erkennung auslöst. Nach dem Vorlesen wird aber
    // direkt weiter aufgenommen (record_until_silence markiert Start/
    // Ende dieser Runde bereits per Ton), damit eine Folgeeingabe ohne
    // erneutes Wake-Word möglich ist.
    //
    // Echo-/Doppeltrigger-Schutz: TTS-Wiedergabe darf nur im Zustand
    // SPEAKING laufen - siehe Kommentar bei der analogen Prüfung vor dem
    // Wake-Word-Lauschen weiter oben.
    sm.require(State::Speaking)?;
    if let Err(e) = cancellable(
        shutdown,
        tts::synthesize_and_play(&cfg.tts, &response, tmp_dir),
    )
    .await
    {
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
    let mut chunk: Vec<f32> = Vec::new();

    'outer: loop {
        chunk.clear();
        if !capture.recv_into(&mut chunk).await {
            warn!("Audio-Stream unerwartet beendet");
            break;
        }
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

    let dropped = capture.dropped_samples.load(Ordering::Relaxed);
    if dropped > 0 {
        warn!(
            dropped_samples = dropped,
            "Audio-Samples wegen vollem Ringpuffer verworfen - Aufnahme könnte kleine Lücken enthalten"
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

    #[tokio::test]
    async fn cancellable_returns_the_futures_result_when_it_finishes_first() {
        let flag = AtomicBool::new(false);
        let result: Result<u32> = cancellable(&flag, async { Ok(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    /// Regression: Vorher wurde das Shutdown-Signal nur beim Warten auf das
    /// Wake-Word beachtet - eine laufende Aufnahme, Whisper-Transkription,
    /// ein OpenClaw-Aufruf oder Piper-TTS liefen ungebremst bis zu ihrem
    /// jeweiligen `timeout_secs` weiter (im ungünstigsten Fall bis zu
    /// `max_recording_seconds`, standardmäßig 300s).
    #[tokio::test]
    async fn cancellable_aborts_with_shutdown_requested_once_the_flag_is_set() {
        let flag = AtomicBool::new(true);
        // Ein Future, das ohne das Shutdown-Signal nie von selbst fertig
        // würde - steht stellvertretend für eine noch laufende Aufnahme-
        // /Whisper-/OpenClaw-/TTS-Phase.
        let never_finishes = std::future::pending::<Result<()>>();
        let result = tokio::time::timeout(
            Duration::from_millis(200),
            cancellable(&flag, never_finishes),
        )
        .await
        .expect("cancellable sollte sofort abbrechen, wenn shutdown schon gesetzt ist");
        let err = result.unwrap_err();
        assert!(
            err.is::<ShutdownRequested>(),
            "erwarteter Fehlertyp ShutdownRequested, war aber: {err}"
        );
    }

    /// Genau die Unterscheidung, die `main()` braucht, um bei einem
    /// angeforderten Abbruch weder einen Fehlerton zu spielen noch `error!`
    /// zu loggen.
    #[test]
    fn shutdown_requested_is_distinguishable_from_a_real_error() {
        let shutdown_err: anyhow::Error = ShutdownRequested.into();
        let real_err = anyhow::anyhow!("etwas ist wirklich kaputt");
        assert!(shutdown_err.is::<ShutdownRequested>());
        assert!(!real_err.is::<ShutdownRequested>());
    }
}
