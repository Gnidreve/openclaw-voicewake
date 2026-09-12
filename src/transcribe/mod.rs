//! Transkribiert eine Aufnahme zu Text. Zwei Wege, gesteuert über
//! `openclaw.audio_pipeline` (siehe `config::AudioPipeline`), beide
//! dauerhaft vollwertig unterstützt - keiner ist ein Fallback des anderen:
//!   - [`local`]: ffmpeg-Normalisierung + `whisper-cli`.
//!   - [`gateway`]: eine OpenClaw-Gateway-Talk-Transkriptionssession.

pub mod gateway;
pub mod local;
