use unicode_width::UnicodeWidthChar;

/// Index of the first byte after the escape sequence starting at `index`.
///
/// Only CSI sequences carry parameters, so a bare `\x1b` advances by one and a
/// `\x1b[` run consumes bytes up to and including its alphabetic final byte.
fn skip_ansi_escape(bytes: &[u8], index: usize) -> usize {
    let mut index = index + 1;
    if index < bytes.len() && bytes[index] == b'[' {
        index += 1;
        while index < bytes.len() && !(bytes[index] as char).is_ascii_alphabetic() {
            index += 1;
        }
        index += usize::from(index < bytes.len());
    }
    index
}

pub(crate) fn visible_width(value: &str) -> usize {
    let bytes = value.as_bytes();
    let mut width = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 0x1b {
            index = skip_ansi_escape(bytes, index);
            continue;
        }
        let Some(ch) = value[index..].chars().next() else {
            break;
        };
        width += char_display_width(ch);
        index += ch.len_utf8();
    }
    width
}

fn contains_ansi(value: &str) -> bool {
    value.as_bytes().contains(&0x1b)
}

fn char_display_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}

pub(crate) fn visible_width_max_line(value: &str) -> usize {
    value.lines().map(visible_width).max().unwrap_or_default()
}

/// Shorten `value` to at most `width` display columns, marking the cut with `…`.
///
/// The result never exceeds `width`, so a `width` of zero yields an empty
/// string. Values that already fit are returned unchanged. ANSI escapes are
/// copied through without spending width, and a reset is appended before the
/// ellipsis so a cut inside a colored run cannot leak its style into the rest of
/// the line.
///
/// # Examples
///
/// ```
/// use ccusage_terminal::truncate_to_width;
///
/// assert_eq!(truncate_to_width("fits", 10), "fits");
/// assert_eq!(truncate_to_width("Loading usage logs", 10), "Loading u…");
/// assert_eq!(truncate_to_width("Loading usage logs", 0), "");
/// ```
pub fn truncate_to_width(value: &str, width: usize) -> String {
    if visible_width(value) <= width {
        return value.to_string();
    }
    // The ellipsis itself needs a column, so a zero budget can only stay silent.
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }
    let mut output = String::new();
    let mut current_width = 0;
    let mut index = 0;
    let bytes = value.as_bytes();
    while index < bytes.len() {
        if bytes[index] == 0x1b {
            let start = index;
            index = skip_ansi_escape(bytes, index);
            output.push_str(&value[start..index]);
            continue;
        }
        let Some(ch) = value[index..].chars().next() else {
            break;
        };
        let char_width = char_display_width(ch);
        // Stop one column early so the ellipsis itself stays inside `width`.
        if current_width + char_width >= width {
            break;
        }
        output.push(ch);
        current_width += char_width;
        index += ch.len_utf8();
    }
    if contains_ansi(value) && !output.ends_with("\x1b[0m") {
        output.push_str("\x1b[0m");
    }
    output.push('…');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_width_handles_combining_marks_and_cjk() {
        assert_eq!(visible_width("e\u{0301}"), 1);
        assert_eq!(visible_width("表"), 2);
    }

    #[test]
    fn truncate_to_width_keeps_values_that_already_fit() {
        assert_eq!(
            truncate_to_width("Loading usage logs", 40),
            "Loading usage logs"
        );
    }

    #[test]
    fn truncate_to_width_stays_within_the_requested_width() {
        assert_eq!(truncate_to_width("Loading usage logs", 10), "Loading u…");
    }

    #[test]
    fn truncate_to_width_writes_nothing_when_no_column_is_available() {
        assert_eq!(truncate_to_width("Loading usage logs", 0), "");
        assert_eq!(truncate_to_width("Loading usage logs", 1), "…");
    }

    #[test]
    fn truncate_to_width_never_splits_a_wide_char_across_the_boundary() {
        assert_eq!(truncate_to_width("表表表表", 5), "表表…");
    }

    #[test]
    fn truncate_to_width_preserves_ansi_reset() {
        let truncated = truncate_to_width("\x1b[33mvery-long-value\x1b[0m", 8);

        assert!(truncated.ends_with("\x1b[0m…"));
    }

    #[test]
    fn snapshots_ansi_truncation_boundary() {
        insta::assert_snapshot!(truncate_to_width("\x1b[33mvery-long-value\x1b[0m", 8));
    }

    #[test]
    fn char_display_width_handles_standard_width_cases() {
        assert_eq!(char_display_width('a'), 1);
        assert_eq!(char_display_width('表'), 2);
        assert_eq!(char_display_width('\u{0301}'), 0);
        assert_eq!(char_display_width('\x07'), 0);
        assert_eq!(char_display_width('±'), 1);
    }
}
