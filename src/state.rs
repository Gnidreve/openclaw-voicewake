use std::fmt;
use thiserror::Error;
use tracing::info;

/// Zustände des Sprachdienstes.
///
/// Normaler Zyklus:
/// IDLE -> LISTENING_FOR_WAKEWORD -> RECORDING -> TRANSCRIBING
///      -> SENDING_TO_OPENCLAW -> SPEAKING -> IDLE
///
/// Nach einer vorgelesenen Antwort kann SPEAKING statt nach IDLE auch
/// direkt zurück nach RECORDING springen: Der Kanal bleibt für eine
/// Folgeeingabe offen, ohne dass das Wake-Word erneut nötig ist. Bricht
/// diese Folgeaufnahme ohne erkannte Sprache ab, geht es von dort wie
/// gewohnt nach IDLE - das Wake-Word wird dann wieder benötigt.
///
/// Jeder Zustand außer IDLE selbst kann bei Fehlern/Timeouts direkt nach
/// IDLE zurückspringen (Recovery-Pfad), damit der Dienst nach einem
/// fehlgeschlagenen Schritt nicht hängen bleibt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    Idle,
    ListeningForWakeword,
    Recording,
    Transcribing,
    SendingToOpenClaw,
    Speaking,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            State::Idle => "IDLE",
            State::ListeningForWakeword => "LISTENING_FOR_WAKEWORD",
            State::Recording => "RECORDING",
            State::Transcribing => "TRANSCRIBING",
            State::SendingToOpenClaw => "SENDING_TO_OPENCLAW",
            State::Speaking => "SPEAKING",
        };
        write!(f, "{s}")
    }
}

impl State {
    fn allowed_next(&self) -> &'static [State] {
        use State::*;
        match self {
            Idle => &[ListeningForWakeword],
            ListeningForWakeword => &[Recording, Idle],
            Recording => &[Transcribing, Idle],
            Transcribing => &[SendingToOpenClaw, Idle],
            SendingToOpenClaw => &[Speaking, Idle],
            // Recording: Folgeeingabe ohne erneutes Wake-Word.
            Speaking => &[Idle, Recording],
        }
    }
}

#[derive(Debug, Error)]
pub enum StateError {
    #[error("Ungültiger Zustandsübergang: {from} -> {to}")]
    InvalidTransition { from: State, to: State },
}

pub struct StateMachine {
    current: State,
}

impl StateMachine {
    pub fn new() -> Self {
        Self {
            current: State::Idle,
        }
    }

    pub fn current(&self) -> State {
        self.current
    }

    pub fn transition(&mut self, to: State) -> Result<(), StateError> {
        if !self.current.allowed_next().contains(&to) {
            return Err(StateError::InvalidTransition {
                from: self.current,
                to,
            });
        }
        info!(from = %self.current, to = %to, "Zustandswechsel");
        self.current = to;
        Ok(())
    }
}

impl Default for StateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use State::*;

    #[test]
    fn full_happy_path_cycle() {
        let mut sm = StateMachine::new();
        assert_eq!(sm.current(), Idle);
        for s in [
            ListeningForWakeword,
            Recording,
            Transcribing,
            SendingToOpenClaw,
            Speaking,
            Idle,
        ] {
            sm.transition(s).unwrap();
            assert_eq!(sm.current(), s);
        }
    }

    #[test]
    fn invalid_transition_is_rejected_and_state_unchanged() {
        let mut sm = StateMachine::new();
        let err = sm.transition(Recording).unwrap_err();
        assert!(matches!(
            err,
            StateError::InvalidTransition {
                from: Idle,
                to: Recording
            }
        ));
        assert_eq!(sm.current(), Idle);
    }

    #[test]
    fn every_intermediate_state_can_recover_to_idle() {
        let paths: [(&[State], State); 4] = [
            (&[], ListeningForWakeword),
            (&[ListeningForWakeword], Recording),
            (&[ListeningForWakeword, Recording], Transcribing),
            (
                &[ListeningForWakeword, Recording, Transcribing],
                SendingToOpenClaw,
            ),
        ];
        for (prefix, target) in paths {
            let mut sm = StateMachine::new();
            for &p in prefix {
                sm.transition(p).unwrap();
            }
            sm.transition(target).unwrap();
            sm.transition(Idle).unwrap();
            assert_eq!(sm.current(), Idle);
        }
    }

    #[test]
    fn speaking_can_go_to_idle_or_recording_but_nothing_else() {
        let mut sm = StateMachine::new();
        for s in [
            ListeningForWakeword,
            Recording,
            Transcribing,
            SendingToOpenClaw,
            Speaking,
        ] {
            sm.transition(s).unwrap();
        }
        assert!(sm.transition(Transcribing).is_err());
        assert!(sm.transition(SendingToOpenClaw).is_err());
        assert_eq!(sm.current(), Speaking);

        assert!(sm.transition(Recording).is_ok());
        assert_eq!(sm.current(), Recording);
    }

    #[test]
    fn multi_turn_conversation_can_loop_back_to_recording_before_idle() {
        let mut sm = StateMachine::new();
        for s in [
            ListeningForWakeword,
            Recording,
            Transcribing,
            SendingToOpenClaw,
            Speaking,
            // Folgeeingabe ohne erneutes Wake-Word:
            Recording,
            Transcribing,
            SendingToOpenClaw,
            Speaking,
            Idle,
        ] {
            sm.transition(s).unwrap();
            assert_eq!(sm.current(), s);
        }
    }
}
