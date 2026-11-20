use std::io;

use crate::{
    style::{Color, TerminalStyle, color},
    width::visible_width,
};

pub fn print_box_title(title: &str, style: impl Into<TerminalStyle>) -> io::Result<()> {
    let style = style.into();
    if style.log_level == Some(0) {
        return Ok(());
    }
    for line in box_title_lines(title, style) {
        crate::outln!("{line}")?;
    }
    Ok(())
}

fn box_title_lines(title: &str, style: TerminalStyle) -> Vec<String> {
    let title_lines = title.lines().collect::<Vec<_>>();
    let content_width = title_lines
        .iter()
        .map(|line| visible_width(line))
        .max()
        .unwrap_or_default()
        .max(40)
        + 2;
    let mut lines = Vec::with_capacity(title_lines.len() + 5);
    lines.push(String::new());
    lines.push(format!("╭{}╮", "─".repeat(content_width + 2)));
    lines.push(format!("│{}│", " ".repeat(content_width + 2)));
    for line in title_lines {
        let padding = content_width.saturating_sub(visible_width(line));
        let left_padding = padding / 2;
        let right_padding = padding - left_padding;
        lines.push(format!(
            "│ {}{}{} │",
            " ".repeat(left_padding),
            color(style, line, Color::Blue),
            " ".repeat(right_padding)
        ));
    }
    lines.push(format!("│{}│", " ".repeat(content_width + 2)));
    lines.push(format!("╰{}╯", "─".repeat(content_width + 2)));
    lines.push(String::new());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_title_lines_render_multiline_content_in_one_box() {
        let lines = box_title_lines(
            "Coding (Agent) CLI Usage Report - Daily\nDetected: Claude, Codex",
            TerminalStyle {
                no_color: true,
                ..TerminalStyle::default()
            },
        );

        assert!(
            lines
                .iter()
                .any(|line| line.contains("Coding (Agent) CLI Usage Report - Daily"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Detected: Claude, Codex"))
        );
        assert_eq!(lines.iter().filter(|line| line.starts_with('╭')).count(), 1);
        assert_eq!(lines.iter().filter(|line| line.starts_with('╰')).count(), 1);
    }

    #[test]
    fn snapshots_multiline_box_title_layout() {
        let lines = box_title_lines(
            "Coding (Agent) CLI Usage Report - Daily\nDetected: Claude, Codex",
            TerminalStyle {
                no_color: true,
                ..TerminalStyle::default()
            },
        );

        insta::assert_snapshot!(lines.join("\n"));
    }
}
