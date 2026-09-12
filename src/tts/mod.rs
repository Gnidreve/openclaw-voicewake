//! Spricht einen Antworttext. Zwei Wege, gesteuert über
//! `openclaw.audio_pipeline` (siehe `config::AudioPipeline`) - demselben
//! Schalter wie bei `transcribe`, nur für die Ausgaberichtung (ROADMAP.md:
//! "derselbe `audio_pipeline = \"gateway\"`-Schalter"). Beide dauerhaft
//! vollwertig unterstützt, keiner ist ein Fallback des anderen:
//!   - [`local`]: Piper-Aufruf + Wiedergabe.
//!   - [`gateway`]: `tts.speak` gegen das OpenClaw-Gateway.

pub mod gateway;
pub mod local;
