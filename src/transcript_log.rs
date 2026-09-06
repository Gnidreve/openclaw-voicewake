use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tracing::warn;

use crate::config::TranscriptionLogConfig;

/// Chat-artiges Diagnose-Log ("[Input] ..." / "[Output] ...") zusätzlich
/// zu den strukturierten `tracing`-Logs. Nachrichten in Anführungszeichen
/// sind ein erfolgreich übermittelter/vorgelesener Text; alles andere
/// (`skipped`, `error`) ist eine Statusmeldung für einen nicht
/// erfolgreichen Schritt.
pub enum OutputOutcome<'a> {
    Success(&'a str),
    Skipped,
    Error,
}

fn format_input_line(transcript: &str) -> String {
    let trimmed = transcript.trim();
    if trimmed.is_empty() {
        "[Input] skipped".to_string()
    } else {
        format!("[Input] \"{trimmed}\"")
    }
}

/// Bewusst **ohne** Anführungszeichen: der Text wurde gerade nicht als
/// Eingabe übermittelt. Er steht trotzdem in der Zeile, weil sonst nicht
/// nachvollziehbar wäre, was der Filter verworfen hat.
fn format_ignored_input_line(transcript: &str) -> String {
    format!("[Input] ignored: {}", transcript.trim())
}

fn format_output_line(outcome: &OutputOutcome<'_>) -> String {
    match outcome {
        OutputOutcome::Success(text) => format!("[Output] \"{}\"", text.trim()),
        OutputOutcome::Skipped => "[Output] skipped".to_string(),
        OutputOutcome::Error => "[Output] error".to_string(),
    }
}

pub async fn log_input(cfg: &TranscriptionLogConfig, transcript: &str) {
    write_line(cfg, &format_input_line(transcript)).await;
}

/// Für ein Transkript, das der Transkript-Filter als Störgeräusch/
/// Halluzination verworfen hat (siehe `transcript_filter`).
pub async fn log_input_ignored(cfg: &TranscriptionLogConfig, transcript: &str) {
    write_line(cfg, &format_ignored_input_line(transcript)).await;
}

pub async fn log_output(cfg: &TranscriptionLogConfig, outcome: OutputOutcome<'_>) {
    write_line(cfg, &format_output_line(&outcome)).await;
}

/// Markiert im Log den Beginn einer neuen OpenClaw-Session nach einem
/// Session-Reset (siehe `openclaw::reset_due`) - sonst sieht man im Log
/// nicht, ab wo der Kontext der vorherigen Session nicht mehr gilt. Die
/// leere Zeile davor trennt optisch vom vorherigen Block.
pub async fn log_session_reset(cfg: &TranscriptionLogConfig) {
    write_line(cfg, "").await;
    write_line(cfg, "----------- [NEW] ------------").await;
}

async fn write_line(cfg: &TranscriptionLogConfig, line: &str) {
    if !cfg.enabled {
        return;
    }
    if let Err(e) = append(cfg, line).await {
        warn!(error = %e, path = %cfg.path.display(), "Konnte Transcription-Log nicht schreiben");
    }
}

async fn append(cfg: &TranscriptionLogConfig, line: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&cfg.path)
        .await?;
    file.write_all(line.as_bytes()).await?;
    file.write_all(b"\n").await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: Ohne eine sichtbare Markierung sieht man im Log nicht,
    /// ab wo ein Session-Reset den bisherigen Kontext beendet hat.
    #[tokio::test]
    async fn session_reset_writes_a_blank_line_and_a_new_marker() {
        let path = std::env::temp_dir().join(format!(
            "openclaw-voicebridge-test-transcript-log-{}.log",
            uuid::Uuid::new_v4()
        ));
        let cfg = TranscriptionLogConfig {
            enabled: true,
            path: path.clone(),
        };
        log_input(&cfg, "davor").await;
        log_session_reset(&cfg).await;
        log_input(&cfg, "danach").await;

        let content = tokio::fs::read_to_string(&path)
            .await
            .expect("Log sollte lesbar sein");
        let _ = std::fs::remove_file(&path);

        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(
            lines,
            vec![
                "[Input] \"davor\"",
                "",
                "----------- [NEW] ------------",
                "[Input] \"danach\"",
            ]
        );
    }

    #[test]
    fn input_line_shows_transcript_in_quotes() {
        assert_eq!(format_input_line("Hallo"), "[Input] \"Hallo\"");
    }

    #[test]
    fn input_line_trims_whitespace() {
        assert_eq!(format_input_line("  Hallo  "), "[Input] \"Hallo\"");
    }

    #[test]
    fn input_line_shows_skipped_for_empty_transcript() {
        assert_eq!(format_input_line(""), "[Input] skipped");
        assert_eq!(format_input_line("   "), "[Input] skipped");
    }

    #[test]
    fn ignored_input_line_names_the_discarded_text_without_quotes() {
        let line = format_ignored_input_line("  Untertitelung des ZDF, 2020 ");
        assert_eq!(line, "[Input] ignored: Untertitelung des ZDF, 2020");
        assert!(
            !line.contains('"'),
            "verworfener Text darf nicht wie eine übermittelte Eingabe aussehen"
        );
    }

    #[test]
    fn output_line_shows_response_in_quotes() {
        assert_eq!(
            format_output_line(&OutputOutcome::Success("Auch Hallo")),
            "[Output] \"Auch Hallo\""
        );
    }

    #[test]
    fn output_line_shows_skipped() {
        assert_eq!(
            format_output_line(&OutputOutcome::Skipped),
            "[Output] skipped"
        );
    }

    #[test]
    fn output_line_shows_error() {
        assert_eq!(format_output_line(&OutputOutcome::Error), "[Output] error");
    }
}
