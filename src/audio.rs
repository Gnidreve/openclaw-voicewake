use anyhow::{bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapRb};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;
use tracing::{error, info};

/// Kapazität des Ringpuffers zwischen Audio-Callback und Verarbeitungs-Task,
/// in Sekunden bei der tatsächlichen Sample-Rate/Kanalzahl des Geräts.
/// Bewusst großzügig bemessen, um kurze Verarbeitungs-Hänger (Scheduling-
/// Jitter der Verarbeitungs-Task) abzufedern, ohne dass der Echtzeit-Audio-
/// Thread blockiert oder Samples verworfen werden müssen.
const RING_BUFFER_SECONDS: usize = 2;

/// Scratch-Puffer auf dem Stack, über den Samples aus dem Ringpuffer gelesen
/// bzw. (bei I16) vor dem Schreiben in ihn konvertiert werden - bewusst fest
/// und ohne jede Heap-Allokation im Echtzeit-Pfad.
const AUDIO_SCRATCH_LEN: usize = 1024;

/// `i16::MAX as f32` einmalig vorberechnet, um die Konvertierung nicht bei
/// jedem einzelnen Sample erneut auszuführen.
const I16_MAX_F32: f32 = i16::MAX as f32;
/// `1.0 / i16::MAX` einmalig vorberechnet statt bei jeder Konvertierung
/// erneut `i16::MAX as f32` zu berechnen und zu dividieren.
const I16_TO_F32_SCALE: f32 = 1.0 / I16_MAX_F32;

/// Konvertiert ein i16-PCM-Sample nach f32 im Bereich -1.0..=1.0.
#[inline]
fn i16_to_f32(sample: i16) -> f32 {
    sample as f32 * I16_TO_F32_SCALE
}

/// Hält den laufenden CoreAudio-Input-Stream sowie die Konsumenten-Seite des
/// Ringpuffers für PCM-Samples (f32, interleaved über alle Kanäle).
pub struct AudioCapture {
    _stream: cpal::Stream,
    consumer: HeapCons<f32>,
    /// Weckt die wartende Aufnahmeschleife, sobald der Callback neue Samples
    /// geschrieben hat oder der Stream einen Fehler gemeldet hat.
    notify: Arc<Notify>,
    /// Gesetzt vom Fehler-Callback des Streams - es sind keine weiteren
    /// Samples mehr zu erwarten.
    ended: Arc<AtomicBool>,
    pub sample_rate: u32,
    pub channels: u16,
    /// Anzahl der wegen vollem Ringpuffer verworfenen Samples seit Start der
    /// Aufnahme (siehe Kommentar bei `push_slice` weiter unten).
    pub dropped_samples: Arc<AtomicU64>,
}

impl AudioCapture {
    /// Wartet, bis mindestens ein Sample verfügbar ist, und hängt alle
    /// aktuell im Ringpuffer liegenden Samples an `out` an. Gibt `false`
    /// zurück, wenn der Stream beendet ist und keine weiteren Daten mehr zu
    /// erwarten sind (`out` bleibt dann unverändert).
    pub async fn recv_into(&mut self, out: &mut Vec<f32>) -> bool {
        drain_or_wait(&mut self.consumer, &self.notify, &self.ended, out).await
    }
}

/// Kern von [`AudioCapture::recv_into`], ohne Kopplung an `cpal::Stream` -
/// dadurch ohne echtes Audiogerät testbar (siehe `tests` unten).
async fn drain_or_wait(
    consumer: &mut HeapCons<f32>,
    notify: &Notify,
    ended: &AtomicBool,
    out: &mut Vec<f32>,
) -> bool {
    loop {
        let before = out.len();
        let mut scratch = [0f32; AUDIO_SCRATCH_LEN];
        loop {
            let n = consumer.pop_slice(&mut scratch);
            if n == 0 {
                break;
            }
            out.extend_from_slice(&scratch[..n]);
        }
        if out.len() > before {
            return true;
        }
        if ended.load(Ordering::Relaxed) {
            return false;
        }
        notify.notified().await;
    }
}

/// Startet die Mikrofonaufnahme. Liefert einen klaren Fehler (statt eines
/// Absturzes), wenn kein Eingabegerät gefunden wird oder der Stream nicht
/// aufgebaut werden kann (z. B. fehlende Mikrofonberechtigung unter macOS).
pub fn start_capture(device_name: Option<&str>) -> Result<AudioCapture> {
    let host = cpal::default_host();

    let device = match device_name {
        Some(name) => host
            .input_devices()
            .context("Kann Audio-Eingabegeräte nicht auflisten")?
            .find(|d| d.name().map(|n| n == name).unwrap_or(false))
            .ok_or_else(|| anyhow::anyhow!("Audiogerät '{name}' nicht gefunden"))?,
        None => host.default_input_device().ok_or_else(|| {
            anyhow::anyhow!(
                "Kein Mikrofon gefunden. Bitte ein Eingabegerät anschließen, die \
                 Mikrofonberechtigung in Systemeinstellungen > Datenschutz & Sicherheit \
                 > Mikrofon prüfen, oder --dry-run verwenden."
            )
        })?,
    };

    let dev_name = device.name().unwrap_or_else(|_| "unbekannt".into());
    info!(device = %dev_name, "Verwende Audiogerät");

    let supported = device.default_input_config().context(
        "Kann Standard-Eingabekonfiguration nicht ermitteln (evtl. fehlende Mikrofonberechtigung)",
    )?;

    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let sample_rate = config.sample_rate.0;
    let channels = config.channels;

    let ring_capacity = (sample_rate as usize) * (channels as usize) * RING_BUFFER_SECONDS;
    let (producer, consumer) = HeapRb::<f32>::new(ring_capacity.max(1)).split();

    let notify = Arc::new(Notify::new());
    let ended = Arc::new(AtomicBool::new(false));
    // Zählt verworfene Samples bei vollem Ringpuffer, um das im Empfänger-
    // Task gebündelt (statt pro Sample) zu loggen - Logging selbst darf im
    // Echtzeit-Callback ebenfalls nicht passieren.
    let dropped = Arc::new(AtomicU64::new(0));

    let err_fn = {
        let ended = ended.clone();
        let notify = notify.clone();
        move |err| {
            error!(error = %err, "Fehler im Audio-Stream");
            // Weckt eine wartende Aufnahmeschleife sofort, statt dass sie bis
            // zum nächsten (dann nie kommenden) Sample hängen bleibt.
            ended.store(true, Ordering::Relaxed);
            notify.notify_one();
        }
    };

    let stream = if sample_format == SampleFormat::F32 {
        let mut producer = producer;
        let dropped = dropped.clone();
        let notify = notify.clone();
        device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    // WICHTIG: push_slice statt einer Heap-Allokation (früher
                    // `data.to_vec()` + `try_send`). Dieser Callback läuft auf
                    // dem Echtzeit-Audio-Thread von CoreAudio - eine
                    // Allokation dort kann auf den Allocator warten und damit
                    // Dropouts/Knacken im Audiosignal riskieren. Ist der
                    // Ringpuffer voll, werden die überzähligen Samples
                    // verworfen statt den Thread anzuhalten.
                    let written = producer.push_slice(data);
                    if written < data.len() {
                        dropped.fetch_add((data.len() - written) as u64, Ordering::Relaxed);
                    }
                    notify.notify_one();
                },
                err_fn,
                None,
            )
            .context("Kann Audio-Eingabe-Stream nicht erstellen")?
    } else if sample_format == SampleFormat::I16 {
        let mut producer = producer;
        let dropped = dropped.clone();
        let notify = notify.clone();
        device
            .build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    // Konvertierung über einen Stack-Scratch-Puffer statt
                    // `.map(...).collect()` in einen neuen Vec - aus demselben
                    // Grund wie oben keine Heap-Allokation im Callback.
                    let mut scratch = [0f32; AUDIO_SCRATCH_LEN];
                    for chunk in data.chunks(AUDIO_SCRATCH_LEN) {
                        for (dst, &s) in scratch.iter_mut().zip(chunk) {
                            *dst = i16_to_f32(s);
                        }
                        let written = producer.push_slice(&scratch[..chunk.len()]);
                        if written < chunk.len() {
                            dropped.fetch_add((chunk.len() - written) as u64, Ordering::Relaxed);
                        }
                    }
                    notify.notify_one();
                },
                err_fn,
                None,
            )
            .context("Kann Audio-Eingabe-Stream nicht erstellen")?
    } else {
        bail!("Nicht unterstütztes Sample-Format: {sample_format:?} (unterstützt: F32, I16)");
    };

    stream.play().context("Kann Audio-Stream nicht starten")?;

    Ok(AudioCapture {
        _stream: stream,
        consumer,
        notify,
        ended,
        sample_rate,
        channels,
        dropped_samples: dropped,
    })
}

/// Schreibt f32-Samples (-1.0..1.0) als 16-bit PCM WAV.
pub fn write_wav(path: &Path, samples: &[f32], sample_rate: u32, channels: u16) -> Result<()> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("Kann WAV-Datei nicht erstellen: {}", path.display()))?;
    for &s in samples {
        let clamped = (s.clamp(-1.0, 1.0) * I16_MAX_F32) as i16;
        writer.write_sample(clamped)?;
    }
    writer.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i16_to_f32_maps_full_range_correctly() {
        assert_eq!(i16_to_f32(0), 0.0);
        assert!((i16_to_f32(i16::MAX) - 1.0).abs() < 1e-6);
        // i16::MIN liegt betragsmäßig einen Schritt über i16::MAX, das
        // Ergebnis darf daher knapp unter -1.0 liegen, aber nicht drüber.
        assert!(i16_to_f32(i16::MIN) <= -1.0);
        assert!(i16_to_f32(i16::MIN) > -1.001);
    }

    /// Reproduziert den Callback->Verarbeitung-Pfad ohne echtes Audiogerät:
    /// Producer schreibt (wie im Callback), `drain_or_wait` liest darüber
    /// hinweg, ohne dass dafür ein `cpal::Stream` existieren muss.
    #[tokio::test]
    async fn drains_everything_the_producer_already_pushed() {
        let (mut producer, mut consumer) = HeapRb::<f32>::new(16).split();
        producer.push_slice(&[1.0, 2.0, 3.0]);
        let notify = Notify::new();
        let ended = AtomicBool::new(false);

        let mut out = Vec::new();
        let got_data = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            drain_or_wait(&mut consumer, &notify, &ended, &mut out),
        )
        .await
        .expect("sollte sofort zurückkehren, wenn bereits Daten vorliegen");
        assert!(got_data);
        assert_eq!(out, vec![1.0, 2.0, 3.0]);
    }

    #[tokio::test]
    async fn returns_false_once_the_stream_has_ended_and_is_empty() {
        let (_producer, mut consumer) = HeapRb::<f32>::new(16).split();
        let notify = Notify::new();
        let ended = AtomicBool::new(true);

        let mut out = Vec::new();
        let got_data = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            drain_or_wait(&mut consumer, &notify, &ended, &mut out),
        )
        .await
        .expect("sollte sofort zurückkehren, wenn der Stream schon beendet ist");
        assert!(!got_data);
        assert!(out.is_empty());
    }

    /// Ohne Daten und ohne `ended` muss auf `notify` gewartet werden, statt
    /// mit leerem `out` zurückzukehren (sonst würde die Aufnahmeschleife in
    /// eine Busy-Loop laufen).
    #[tokio::test]
    async fn waits_for_notify_when_the_buffer_is_empty_and_the_stream_is_alive() {
        let (_producer, mut consumer) = HeapRb::<f32>::new(16).split();
        let notify = Notify::new();
        let ended = AtomicBool::new(false);

        let mut out = Vec::new();
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            drain_or_wait(&mut consumer, &notify, &ended, &mut out),
        )
        .await;
        assert!(
            result.is_err(),
            "sollte hängen bleiben, solange kein Sample kommt und der Stream nicht beendet ist"
        );
    }

    #[tokio::test]
    async fn wakes_up_and_drains_once_notified_after_a_late_push() {
        let (mut producer, mut consumer) = HeapRb::<f32>::new(16).split();
        let notify = Arc::new(Notify::new());
        let ended = Arc::new(AtomicBool::new(false));

        let notify_for_task = notify.clone();
        let ended_for_task = ended.clone();
        let handle = tokio::spawn(async move {
            let mut out = Vec::new();
            let got_data =
                drain_or_wait(&mut consumer, &notify_for_task, &ended_for_task, &mut out).await;
            (got_data, out)
        });

        // Gibt der gespawnten Task Zeit, bis zum `notify.notified().await` zu
        // laufen und sich dort als Warter zu registrieren, bevor gepusht und
        // benachrichtigt wird.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        producer.push_slice(&[42.0]);
        notify.notify_one();

        let (got_data, out) = tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("Task sollte nach dem notify zurückkehren")
            .expect("Task darf nicht abbrechen (panic)");
        assert!(got_data);
        assert_eq!(out, vec![42.0]);
    }
}
