//! Filter gegen Transkripte, die keine echte Nutzereingabe sind.
//!
//! Hintergrund aus dem Feldtest: Läuft im Raum ein Fernseher, erkennt die VAD
//! den TV-Ton als Sprache und Whisper macht daraus typische Abspann-/
//! Untertitel-Halluzinationen ("Untertitelung des ZDF, 2020"). Ohne Filter
//! landen die als vollwertige Eingabe bei OpenClaw und halten - weil sie eine
//! Antwort erzeugen - den Kanal für weitere Folgeeingaben offen.

/// Normalisiert Text für den Mustervergleich: Kleinschreibung, Apostrophe
/// entfernt ("für's" == "fürs"), alle übrigen Satz-/Sonderzeichen zu
/// Leerzeichen, Mehrfach-Leerzeichen zusammengefasst.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if matches!(c, '\'' | '\u{2019}' | '\u{02BC}' | '`') {
            continue;
        }
        if c.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.extend(c.to_lowercase());
        } else {
            pending_space = true;
        }
    }
    out
}

/// Gibt das erste Muster zurück, das im Transkript enthalten ist - oder
/// `None`, wenn das Transkript als echte Eingabe durchgehen soll. Ein leeres
/// Transkript oder eine leere Musterliste liefert immer `None`; das
/// bestehende "leeres Transkript"-Verhalten bleibt davon unberührt.
pub fn matching_pattern<'a>(patterns: &'a [String], transcript: &str) -> Option<&'a str> {
    let haystack = normalize(transcript);
    if haystack.is_empty() {
        return None;
    }
    patterns.iter().map(String::as_str).find(|pattern| {
        let needle = normalize(pattern);
        !needle.is_empty() && haystack.contains(&needle)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TranscriptFilterConfig;

    fn defaults() -> Vec<String> {
        TranscriptFilterConfig::default().ignored_patterns
    }

    #[test]
    fn normalize_lowercases_and_strips_punctuation() {
        assert_eq!(
            normalize("Untertitelung des ZDF, 2020"),
            "untertitelung des zdf 2020"
        );
        assert_eq!(normalize("  Hallo   Welt!  "), "hallo welt");
        assert_eq!(normalize("für's"), "fürs");
        assert_eq!(normalize("...!?"), "");
    }

    #[test]
    fn filters_the_zdf_hallucination_from_the_field_test() {
        assert_eq!(
            matching_pattern(&defaults(), "Untertitelung des ZDF, 2020"),
            Some("Untertitelung des ZDF")
        );
    }

    #[test]
    fn filters_amara_and_apostrophe_variants() {
        assert!(matching_pattern(
            &defaults(),
            "Untertitelung aufgrund der Amara.org-Community"
        )
        .is_some());
        assert!(matching_pattern(&defaults(), "Vielen Dank für's Zuschauen.").is_some());
    }

    #[test]
    fn leaves_real_input_alone() {
        for transcript in [
            "Vielen Dank.",
            "Wie spät ist es?",
            "Starte bitte den Build und sag mir Bescheid.",
        ] {
            assert_eq!(
                matching_pattern(&defaults(), transcript),
                None,
                "{transcript}"
            );
        }
    }

    #[test]
    fn empty_transcript_and_empty_pattern_list_never_match() {
        assert_eq!(matching_pattern(&defaults(), ""), None);
        assert_eq!(matching_pattern(&defaults(), "   ...  "), None);
        assert_eq!(matching_pattern(&[], "Untertitelung des ZDF"), None);
    }

    #[test]
    fn blank_patterns_are_ignored_instead_of_matching_everything() {
        let patterns = vec!["".to_string(), "  ".to_string()];
        assert_eq!(matching_pattern(&patterns, "Beliebiger Text"), None);
    }

    #[test]
    fn custom_patterns_match_case_insensitively() {
        let patterns = vec!["Werbung".to_string()];
        assert_eq!(
            matching_pattern(&patterns, "und jetzt kommt WERBUNG"),
            Some("Werbung")
        );
    }
}
