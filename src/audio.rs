use anyhow::{bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};

/// Kapazität des Channels zwischen Audio-Callback und Verarbeitungs-Task.
/// Bewusst großzügig bemessen, um kurze Verarbeitungs-Hänger (GC-freie Rust-
/// Allocator-Pausen, Scheduling-Jitter) abzufedern, ohne dass der
/// Echtzeit-Audio-Thread blockiert oder Samples verworfen werden müssen.
const AUDIO_CHANNEL_CAPACITY: usize = 512;

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

/// Hält den laufenden CoreAudio-Input-Stream sowie den Empfänger für
/// PCM-Chunks (f32, interleaved über alle Kanäle).
pub struct AudioCapture {
    _stream: cpal::Stream,
    pub receiver: mpsc::Receiver<Vec<f32>>,
    pub sample_rate: u32,
    pub channels: u16,
    /// Anzahl der wegen Backpressure verworfenen Audio-Chunks seit Start
    /// der Aufnahme (siehe Kommentar bei `try_send` weiter unten).
    pub dropped_chunks: Arc<AtomicU64>,
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

    let (tx, rx) = mpsc::channel::<Vec<f32>>(AUDIO_CHANNEL_CAPACITY);
    let err_fn = |err| error!(error = %err, "Fehler im Audio-Stream");
    // Zählt verworfene Chunks bei Backpressure, um das im Empfänger-Task
    // gebündelt (statt pro Chunk) zu loggen - Logging selbst darf im
    // Echtzeit-Callback ebenfalls nicht passieren.
    let dropped = Arc::new(AtomicU64::new(0));

    let stream = if sample_format == SampleFormat::F32 {
        let dropped = dropped.clone();
        device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    // WICHTIG: try_send statt blocking_send. Dieser Callback läuft
                    // auf dem Echtzeit-Audio-Thread von CoreAudio - blockieren
                    // (z. B. bei vollem Channel) würde Dropouts/Knacken im
                    // Audiosignal riskieren. Bei Backpressure wird der Chunk
                    // verworfen statt den Thread anzuhalten.
                    if tx.try_send(data.to_vec()).is_err() {
                        dropped.fetch_add(1, Ordering::Relaxed);
                    }
                },
                err_fn,
                None,
            )
            .context("Kann Audio-Eingabe-Stream nicht erstellen")?
    } else if sample_format == SampleFormat::I16 {
        let dropped = dropped.clone();
        device
            .build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let converted: Vec<f32> = data.iter().map(|&s| i16_to_f32(s)).collect();
                    if tx.try_send(converted).is_err() {
                        dropped.fetch_add(1, Ordering::Relaxed);
                    }
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
        receiver: rx,
        sample_rate,
        channels,
        dropped_chunks: dropped,
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
}
