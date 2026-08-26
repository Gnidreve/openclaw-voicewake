use crate::config::VadConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadDecision {
    Continue,
    StopSilence,
    StopMaxDuration,
}

/// Reiner, testbarer Timing-Zustand für die Stille-Erkennung. Nimmt
/// pro Frame einen RMS-Energiewert entgegen und entscheidet, ob die
/// Aufnahme fortgesetzt oder beendet werden soll.
#[derive(Debug)]
pub struct SilenceTracker {
    threshold: f32,
    silence_timeout_ms: u64,
    max_recording_ms: u64,
    min_speech_ms: u64,
    elapsed_ms: u64,
    silence_ms: u64,
    speech_ms: u64,
    speech_started: bool,
}

impl SilenceTracker {
    pub fn new(cfg: &VadConfig) -> Self {
        Self {
            threshold: cfg.silence_rms_threshold,
            silence_timeout_ms: cfg.silence_timeout_ms,
            max_recording_ms: cfg.max_recording_seconds * 1000,
            min_speech_ms: cfg.min_speech_ms,
            elapsed_ms: 0,
            silence_ms: 0,
            speech_ms: 0,
            speech_started: false,
        }
    }

    pub fn push_frame(&mut self, rms: f32, frame_ms: u64) -> VadDecision {
        self.elapsed_ms += frame_ms;

        if rms >= self.threshold {
            self.silence_ms = 0;
            self.speech_ms += frame_ms;
            if self.speech_ms >= self.min_speech_ms {
                self.speech_started = true;
            }
        } else {
            self.silence_ms += frame_ms;
        }

        if self.elapsed_ms >= self.max_recording_ms {
            return VadDecision::StopMaxDuration;
        }

        if self.speech_started && self.silence_ms >= self.silence_timeout_ms {
            return VadDecision::StopSilence;
        }

        VadDecision::Continue
    }

    /// Ob während der Aufnahme jemals RMS-Energie über `silence_rms_threshold`
    /// für mindestens `min_speech_ms` am Stück lag. `false` bedeutet: die
    /// gesamte Aufnahme war (aus VAD-Sicht) Stille/Hintergrundrauschen -
    /// unabhängig davon, was Whisper aus dem Audio heraushalluzinieren würde,
    /// sollte es trotzdem transkribiert werden.
    pub fn speech_started(&self) -> bool {
        self.speech_started
    }
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> VadConfig {
        VadConfig {
            silence_timeout_ms: 4000,
            max_recording_seconds: 60,
            silence_rms_threshold: 0.02,
            min_speech_ms: 300,
            frame_ms: 30,
        }
    }

    #[test]
    fn continues_while_speaking() {
        let mut t = SilenceTracker::new(&cfg());
        for _ in 0..20 {
            assert_eq!(t.push_frame(0.1, 30), VadDecision::Continue);
        }
    }

    #[test]
    fn stops_after_silence_timeout_following_speech() {
        let mut t = SilenceTracker::new(&cfg());
        // genug Sprache füttern, damit speech_started gesetzt wird (>= 300ms)
        for _ in 0..15 {
            assert_eq!(t.push_frame(0.1, 30), VadDecision::Continue);
        }
        // danach Stille bis zum konfigurierten Timeout (4000ms)
        let frames_needed = 4000 / 30 + 2;
        let mut last = VadDecision::Continue;
        for _ in 0..frames_needed {
            last = t.push_frame(0.0, 30);
            if last != VadDecision::Continue {
                break;
            }
        }
        assert_eq!(last, VadDecision::StopSilence);
    }

    #[test]
    fn does_not_stop_on_silence_before_speech_started() {
        let mut t = SilenceTracker::new(&cfg());
        // 500 * 30ms = 15000ms, bleibt unter max_recording_seconds (60s)
        for _ in 0..500 {
            assert_eq!(t.push_frame(0.0, 30), VadDecision::Continue);
        }
    }

    #[test]
    fn stops_on_max_duration_even_while_speaking() {
        let mut c = cfg();
        c.max_recording_seconds = 1; // 1000ms
        c.silence_timeout_ms = 10_000; // hoch genug, damit Stille nicht zuerst greift
        let mut t = SilenceTracker::new(&c);
        let mut last = VadDecision::Continue;
        for _ in 0..40 {
            last = t.push_frame(0.1, 30);
            if last != VadDecision::Continue {
                break;
            }
        }
        assert_eq!(last, VadDecision::StopMaxDuration);
    }

    #[test]
    fn speech_started_stays_false_for_pure_silence() {
        let mut t = SilenceTracker::new(&cfg());
        for _ in 0..50 {
            t.push_frame(0.0, 30);
        }
        assert!(!t.speech_started());
    }

    #[test]
    fn speech_started_becomes_true_once_min_speech_ms_reached() {
        let mut t = SilenceTracker::new(&cfg());
        assert!(!t.speech_started());
        for _ in 0..15 {
            t.push_frame(0.1, 30);
        }
        assert!(t.speech_started());
    }

    #[test]
    fn rms_computes_energy_and_handles_empty_input() {
        let silent = vec![0.0f32; 100];
        let loud = vec![0.5f32; 100];
        assert!(rms(&silent) < rms(&loud));
        assert_eq!(rms(&[]), 0.0);
    }
}
