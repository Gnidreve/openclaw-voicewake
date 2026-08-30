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
    min_threshold: f32,
    noise_floor_margin: f32,
    noise_floor_rise_alpha: f32,
    noise_floor_fall_alpha: f32,
    /// Laufende Schätzung der Umgebungslautstärke, aus jedem Frame
    /// nachgeführt (asymmetrisch, siehe `update_noise_floor`) - unabhängig
    /// davon, ob der Frame als Sprache oder Stille eingestuft wurde.
    noise_floor: f32,
    silence_timeout_ms: u64,
    max_recording_ms: u64,
    min_speech_ms: u64,
    speech_gap_ms: u64,
    elapsed_ms: u64,
    silence_ms: u64,
    speech_ms: u64,
    speech_started: bool,
}

impl SilenceTracker {
    pub fn new(cfg: &VadConfig) -> Self {
        Self {
            min_threshold: cfg.silence_rms_threshold,
            noise_floor_margin: cfg.noise_floor_margin,
            noise_floor_rise_alpha: cfg.noise_floor_rise_alpha,
            noise_floor_fall_alpha: cfg.noise_floor_fall_alpha,
            noise_floor: 0.0,
            silence_timeout_ms: cfg.silence_timeout_ms,
            max_recording_ms: cfg.max_recording_seconds * 1000,
            min_speech_ms: cfg.min_speech_ms,
            speech_gap_ms: cfg.speech_gap_ms,
            elapsed_ms: 0,
            silence_ms: 0,
            speech_ms: 0,
            speech_started: false,
        }
    }

    /// Schwelle für "das ist Sprache" - der höhere Wert aus der fixen
    /// Untergrenze und dem nachgeführten Rauschboden plus Marge. Bei
    /// dauerhaft erhöhter Umgebungslautstärke (laufender Fernseher) steigt
    /// sie mit dem Rauschboden mit, statt dass die Umgebung selbst dauerhaft
    /// als Sprache gilt.
    fn effective_threshold(&self) -> f32 {
        self.min_threshold
            .max(self.noise_floor + self.noise_floor_margin)
    }

    /// Führt den Rauschboden mit `rms` nach - unabhängig von der
    /// Sprache/Stille-Einstufung des Frames. Absichtlich *nicht* an die
    /// Einstufung gekoppelt: Läge die Umgebungslautstärke schon zu Beginn
    /// über der Anfangsschwelle, würde ein Update nur bei "Stille"
    /// eingestuften Frames nie stattfinden - die Schwelle könnte dann nie
    /// nachziehen. Stattdessen sorgt eine asymmetrische Rate dafür, dass
    /// kurze, lautere Abschnitte (Sprache) den Boden nur langsam anheben,
    /// während er nach unten schnell wieder dem tatsächlichen Pegel folgt.
    fn update_noise_floor(&mut self, rms: f32) {
        let alpha = if rms > self.noise_floor {
            self.noise_floor_rise_alpha
        } else {
            self.noise_floor_fall_alpha
        };
        self.noise_floor = alpha * rms + (1.0 - alpha) * self.noise_floor;
    }

    pub fn push_frame(&mut self, rms: f32, frame_ms: u64) -> VadDecision {
        self.elapsed_ms += frame_ms;
        // Schwelle aus dem bisherigen Rauschboden bilden, bevor dieser
        // Frame ihn selbst beeinflusst - sonst würde ein Frame seine eigene
        // Einstufung mitbestimmen.
        let threshold = self.effective_threshold();
        self.update_noise_floor(rms);

        if rms >= threshold {
            self.silence_ms = 0;
            self.speech_ms += frame_ms;
            if self.speech_ms >= self.min_speech_ms {
                self.speech_started = true;
            }
        } else {
            self.silence_ms += frame_ms;
            // `min_speech_ms` misst zusammenhängende Sprache. Ohne diesen
            // Reset addieren sich einzelne laute Frames über die gesamte
            // Aufnahme (bis `max_recording_seconds`) auf, sodass verstreutes
            // Geräusch das Gate öffnet - bei 30-ms-Frames und 300 ms
            // Mindestdauer reichen dafür zehn Frames irgendwo in einer Minute.
            // Kurze Pausen zwischen Silben dürfen den Lauf aber nicht
            // abbrechen, deshalb erst nach `speech_gap_ms` zusammenhängender
            // Stille.
            if self.silence_ms >= self.speech_gap_ms {
                self.speech_ms = 0;
            }
        }

        // Sicherheitsnetz gegen unbegrenztes Wachsen des Aufnahmepuffers, wenn
        // die Umgebung dauerhaft über dem Schwellwert liegt (laufender
        // Fernseher o. Ä.) und die Stille-Uhr deshalb nie abläuft. `0` schaltet
        // es ab. Als Längenbegrenzung für Sprache ist es nicht gedacht.
        if self.max_recording_ms > 0 && self.elapsed_ms >= self.max_recording_ms {
            return VadDecision::StopMaxDuration;
        }

        // Die Stille-Uhr gilt ab dem ersten Frame, nicht erst ab erkannter
        // Sprache: "niemand hat etwas gesagt" endet damit über denselben Weg
        // und nach derselben Zeit wie "jemand hat aufgehört zu sprechen".
        // Sprechen verzögert das Ende nur, indem es die Uhr zurücksetzt.
        if self.silence_ms >= self.silence_timeout_ms {
            return VadDecision::StopSilence;
        }

        VadDecision::Continue
    }

    /// Ob während der Aufnahme jemals RMS-Energie über der jeweils
    /// aktuellen Schwelle (siehe `effective_threshold`) für mindestens
    /// `min_speech_ms` am Stück lag (Pausen bis `speech_gap_ms`
    /// unterbrechen den Lauf nicht). `false` bedeutet: die
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

    const LOUD: f32 = 0.1;
    const QUIET: f32 = 0.0;

    fn cfg() -> VadConfig {
        VadConfig {
            silence_timeout_ms: 4000,
            max_recording_seconds: 60,
            silence_rms_threshold: 0.02,
            noise_floor_margin: 0.02,
            noise_floor_rise_alpha: 0.003,
            noise_floor_fall_alpha: 0.1,
            min_speech_ms: 300,
            speech_gap_ms: 200,
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

    /// Der Gegenpart zu `stops_after_silence_timeout_following_speech`: Ohne
    /// jede Sprache endet die Aufnahme über denselben Weg und nach derselben
    /// Zeit. Früher lief sie stattdessen bis `max_recording_seconds`.
    #[test]
    fn stops_after_the_same_timeout_when_nobody_ever_spoke() {
        let mut t = SilenceTracker::new(&cfg());
        let mut last = VadDecision::Continue;
        let mut elapsed_ms = 0;
        for _ in 0..(4000 / 30 + 2) {
            last = t.push_frame(QUIET, 30);
            if last != VadDecision::Continue {
                break;
            }
            elapsed_ms += 30;
        }
        assert_eq!(last, VadDecision::StopSilence);
        assert!(
            (4000 - 60..=4000).contains(&elapsed_ms),
            "sollte nach ~silence_timeout_ms enden, nicht erst nach max_recording_seconds - war {elapsed_ms} ms"
        );
        assert!(
            !t.speech_started(),
            "reine Stille darf das Sprach-Gate nicht öffnen"
        );
    }

    #[test]
    fn speech_only_postpones_the_same_timeout() {
        let mut t = SilenceTracker::new(&cfg());
        // Kurz vor Ablauf der Uhr sprechen: das verlängert die Aufnahme,
        // beendet sie aber nicht auf einem anderen Weg.
        for _ in 0..130 {
            assert_eq!(t.push_frame(QUIET, 30), VadDecision::Continue);
        }
        assert_eq!(t.push_frame(LOUD, 30), VadDecision::Continue);
        for _ in 0..130 {
            assert_eq!(
                t.push_frame(QUIET, 30),
                VadDecision::Continue,
                "ein einzelnes lautes Frame muss die Uhr komplett zurücksetzen"
            );
        }
    }

    #[test]
    fn max_recording_seconds_zero_disables_the_safety_net() {
        let mut c = cfg();
        c.max_recording_seconds = 0;
        c.silence_timeout_ms = 1_000_000; // damit nur der Deckel greifen könnte
        let mut t = SilenceTracker::new(&c);
        // 20.000 Frames = 600 s, weit über jedem früheren Deckel
        for _ in 0..20_000 {
            assert_eq!(t.push_frame(LOUD, 30), VadDecision::Continue);
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

    /// Regression für den Feldtest-Bug: `speech_ms` wurde nie zurückgesetzt,
    /// also addierten sich einzelne laute Frames über die gesamte Aufnahme zu
    /// "Sprache erkannt" auf. Hier sind es 30 laute Frames (900 ms, also weit
    /// über `min_speech_ms`), aber jeder einzeln zwischen Stille - das ist
    /// Geräusch, keine Sprache.
    #[test]
    fn scattered_loud_frames_never_count_as_speech() {
        let mut t = SilenceTracker::new(&cfg());
        for _ in 0..30 {
            t.push_frame(LOUD, 30);
            // 10 stille Frames = 300 ms > speech_gap_ms
            for _ in 0..10 {
                t.push_frame(QUIET, 30);
            }
        }
        assert!(
            !t.speech_started(),
            "verstreute laute Einzelframes dürfen das Sprach-Gate nicht öffnen"
        );
    }

    /// Der Startton (~1 s über der Schwelle, direkt am Anfang) hat vor dem Fix
    /// jede Aufnahme als "Sprache" markiert. Er kommt jetzt vor dem Öffnen des
    /// Mikrofons - träfe er die Aufnahme doch, wäre er zusammenhängend und
    /// damit auch weiterhin ununterscheidbar von Sprache. Der Test hält
    /// deshalb fest, was der Tracker leisten kann und was nicht: eine
    /// zusammenhängende laute Passage zählt als Sprache.
    #[test]
    fn one_continuous_loud_passage_counts_as_speech() {
        let mut t = SilenceTracker::new(&cfg());
        for _ in 0..10 {
            t.push_frame(LOUD, 30);
        }
        assert!(t.speech_started());
    }

    #[test]
    fn short_pauses_between_syllables_do_not_reset_the_speech_run() {
        let mut t = SilenceTracker::new(&cfg());
        // 5 Frames Sprache (150 ms)
        for _ in 0..5 {
            t.push_frame(LOUD, 30);
        }
        // kurze Silbenpause: 5 * 30 ms = 150 ms < speech_gap_ms (200 ms)
        for _ in 0..5 {
            t.push_frame(QUIET, 30);
        }
        assert!(!t.speech_started(), "150 ms allein reichen noch nicht");
        // weitere 5 Frames Sprache - zusammen 300 ms, der Lauf wurde nicht
        // abgebrochen
        for _ in 0..5 {
            t.push_frame(LOUD, 30);
        }
        assert!(
            t.speech_started(),
            "eine kurze Pause darf den Sprach-Lauf nicht zurücksetzen"
        );
    }

    #[test]
    fn a_long_pause_resets_the_speech_run() {
        let mut t = SilenceTracker::new(&cfg());
        for _ in 0..5 {
            t.push_frame(LOUD, 30);
        }
        // 210 ms Stille >= speech_gap_ms -> Lauf beginnt von vorn
        for _ in 0..7 {
            t.push_frame(QUIET, 30);
        }
        for _ in 0..5 {
            t.push_frame(LOUD, 30);
        }
        assert!(
            !t.speech_started(),
            "nach einer langen Pause zählt min_speech_ms wieder von vorn"
        );
    }

    #[test]
    fn silence_timeout_still_works_after_the_speech_run_was_reset() {
        let mut t = SilenceTracker::new(&cfg());
        for _ in 0..15 {
            t.push_frame(LOUD, 30);
        }
        assert!(t.speech_started());

        // Der Reset von speech_ms darf speech_started nicht zurücknehmen -
        // sonst würde der Stille-Timeout nie greifen.
        let mut last = VadDecision::Continue;
        for _ in 0..(4000 / 30 + 2) {
            last = t.push_frame(QUIET, 30);
            if last != VadDecision::Continue {
                break;
            }
        }
        assert_eq!(last, VadDecision::StopSilence);
        assert!(t.speech_started());
    }

    #[test]
    fn rms_computes_energy_and_handles_empty_input() {
        let silent = vec![0.0f32; 100];
        let loud = vec![0.5f32; 100];
        assert!(rms(&silent) < rms(&loud));
        assert_eq!(rms(&[]), 0.0);
    }

    /// Regression für den eigentlichen Feldtest-Bug: liegt die
    /// Umgebungslautstärke (laufender Fernseher) dauerhaft über dem fixen
    /// `silence_rms_threshold`, erkennt die VAD nie "Stille" und die
    /// Aufnahme lief bis `max_recording_seconds` weiter. Nach genug Frames
    /// mit demselben Pegel muss der nachgeführte Rauschboden die Schwelle
    /// so weit angehoben haben, dass genau dieser Pegel als Stille zählt
    /// und der reguläre Stille-Timeout greift.
    #[test]
    fn sustained_noise_above_the_fixed_threshold_eventually_counts_as_silence() {
        const TV_NOISE: f32 = 0.03; // über dem fixen silence_rms_threshold (0.02)
        let mut c = cfg();
        c.noise_floor_rise_alpha = 0.05; // schneller adaptieren, für einen kompakten Test
        let mut t = SilenceTracker::new(&c);

        // Genug Frames füttern, bis der Rauschboden nachgezogen hat und
        // TV_NOISE nicht mehr über der Schwelle liegt.
        let mut last = VadDecision::Continue;
        for _ in 0..300 {
            last = t.push_frame(TV_NOISE, 30);
            if last != VadDecision::Continue {
                break;
            }
        }

        assert_eq!(
            last,
            VadDecision::StopSilence,
            "TV-Rauschen sollte nach der Anpassung als Stille enden, nicht als max_recording_seconds erreichen"
        );
    }

    /// Ohne die Anfangsschwelle als Untergrenze (`silence_rms_threshold`)
    /// könnte der Rauschboden in einem sehr leisen Raum gegen 0 fallen und
    /// dadurch minimales Mikrofon-Grundrauschen schon als Sprache zählen.
    #[test]
    fn effective_threshold_never_drops_below_the_fixed_minimum() {
        let mut c = cfg();
        c.silence_rms_threshold = 0.02;
        c.noise_floor_margin = 0.005;
        let mut t = SilenceTracker::new(&c);
        for _ in 0..500 {
            t.push_frame(0.0, 30);
        }
        assert!(
            t.effective_threshold() >= 0.02,
            "Schwelle darf die fixe Untergrenze nie unterschreiten, war {}",
            t.effective_threshold()
        );
    }

    /// Eine normal lange Äußerung darf den Rauschboden nicht so weit
    /// anheben, dass sie sich selbst gegen Ende als Stille einstuft - die
    /// Anhebung pro Sekunde muss klein gegenüber einer typischen
    /// Sprechdauer sein.
    #[test]
    fn a_realistic_utterance_does_not_get_swallowed_by_its_own_noise_floor_rise() {
        let mut t = SilenceTracker::new(&cfg());
        // 3 Sekunden zusammenhängende Sprache bei einem für Sprache
        // typischen Pegel, deutlich über der Schwelle.
        for _ in 0..100 {
            assert_eq!(
                t.push_frame(0.2, 30),
                VadDecision::Continue,
                "eine normale, wenige Sekunden lange Äußerung darf nicht vorzeitig als Stille gelten"
            );
        }
        assert!(t.speech_started());
    }

    /// Nach einem lauten Abschnitt (der den Boden minimal anhebt) muss
    /// dieser dank der schnellen Fall-Rate zügig wieder auf den echten,
    /// leisen Pegel zurückfallen - kein dauerhaft erhöhter Boden nach
    /// Sprachende.
    #[test]
    fn noise_floor_recovers_quickly_after_a_loud_section_ends() {
        let mut t = SilenceTracker::new(&cfg());
        for _ in 0..50 {
            t.push_frame(LOUD, 30);
        }
        let floor_after_speech = t.effective_threshold();

        for _ in 0..50 {
            t.push_frame(QUIET, 30);
        }
        assert!(
            t.effective_threshold() < floor_after_speech,
            "Schwelle sollte nach Stille wieder Richtung Minimum gefallen sein"
        );
        assert!(
            (t.effective_threshold() - 0.02).abs() < 0.001,
            "Schwelle sollte nahe an die fixe Untergrenze zurückgefallen sein, war {}",
            t.effective_threshold()
        );
    }
}
