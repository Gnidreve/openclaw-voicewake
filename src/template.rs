//! Platzhalter-Ersetzung für konfigurierbare Argumentlisten (`openclaw.args`,
//! `tts.args`, `openclaw.message_template`).
//!
//! Verwendet wird ein einziger Links-nach-rechts-Durchlauf statt mehrerer
//! nacheinander ausgeführter `str::replace()`-Aufrufe: Bei verketteten
//! `.replace()`-Aufrufen kann ein bereits eingesetzter Wert (z. B.
//! `target_channel`) selbst den literalen Text eines später ersetzten
//! Platzhalters enthalten und würde dann vom nächsten `.replace()`
//! fälschlich noch einmal ersetzt. Da dieser Durchlauf nur einmal über die
//! Eingabe läuft und bereits eingesetzten Text nie erneut scannt, kann das
//! nicht passieren.

/// Ersetzt mehrere Platzhalter in `input` in einem einzigen Durchlauf.
pub fn substitute(input: &str, replacements: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    'outer: while !rest.is_empty() {
        for (placeholder, value) in replacements {
            if let Some(stripped) = rest.strip_prefix(placeholder) {
                out.push_str(value);
                rest = stripped;
                continue 'outer;
            }
        }
        let mut chars = rest.chars();
        out.push(chars.next().expect("rest ist an dieser Stelle nicht leer"));
        rest = chars.as_str();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_a_single_placeholder() {
        assert_eq!(
            substitute("hallo {name}", &[("{name}", "Welt")]),
            "hallo Welt"
        );
    }

    #[test]
    fn substitutes_multiple_placeholders_in_one_pass() {
        assert_eq!(substitute("{a}-{b}", &[("{a}", "1"), ("{b}", "2")]), "1-2");
    }

    /// Regression: Bei verketteten `.replace()`-Aufrufen konnte ein bereits
    /// eingesetzter Wert selbst einen später ersetzten Platzhalter enthalten
    /// und wurde dann fälschlich noch einmal ersetzt. Ein einziger Durchlauf
    /// darf über einen gerade erst eingesetzten Wert nicht erneut laufen.
    #[test]
    fn a_substituted_value_containing_another_placeholders_literal_text_is_not_re_substituted() {
        let result = substitute("{a}-{b}", &[("{a}", "chan-{b}-x"), ("{b}", "message")]);
        assert_eq!(result, "chan-{b}-x-message");
    }

    #[test]
    fn placeholders_work_inside_a_combined_argument() {
        assert_eq!(
            substitute("--output-file={output}", &[("{output}", "/tmp/out.wav")]),
            "--output-file=/tmp/out.wav"
        );
    }

    #[test]
    fn input_without_any_placeholder_is_passed_through_unchanged() {
        assert_eq!(
            substitute("--quiet", &[("{output}", "/tmp/out.wav")]),
            "--quiet"
        );
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert_eq!(substitute("", &[("{a}", "1")]), "");
    }
}
