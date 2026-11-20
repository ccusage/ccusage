use crate::width::{ansi_continuation, truncate_to_width, visible_width};

pub(super) fn expand_multiline_row(
    row: &[String],
    column_count: usize,
    widths: &[usize],
) -> Vec<Vec<String>> {
    let cells = (0..column_count)
        .map(|index| {
            let content_width = widths
                .get(index)
                .copied()
                .unwrap_or_default()
                .saturating_sub(2);
            row.get(index)
                .map(|cell| wrap_cell_lines(cell, content_width))
                .filter(|lines| !lines.is_empty())
                .unwrap_or_else(|| vec![String::new()])
        })
        .collect::<Vec<_>>();
    let height = cells.iter().map(Vec::len).max().unwrap_or(1);
    (0..height)
        .map(|line_index| {
            cells
                .iter()
                .map(|lines| lines.get(line_index).cloned().unwrap_or_default())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn wrap_cell_lines(cell: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    for line in cell.lines() {
        if visible_width(line) <= width {
            lines.push(line.to_string());
        } else {
            lines.extend(wrap_cell_line(line, width));
        }
    }
    lines
}

fn wrap_cell_line(line: &str, width: usize) -> Vec<String> {
    let mut remaining = line.split_whitespace();
    let mut words = Vec::new();
    while let Some(word) = remaining.next() {
        if is_list_marker(word) {
            words.push(
                remaining
                    .next()
                    .map_or_else(|| word.to_string(), |next| format!("{word} {next}")),
            );
        } else {
            words.push(word.to_string());
        }
    }
    if words.len() <= 1 {
        return vec![truncate_to_width(line, width)];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_source = String::new();
    for word in words {
        let candidate_width = if current.is_empty() {
            visible_width(&word)
        } else {
            visible_width(&current) + 1 + visible_width(&word)
        };
        if candidate_width <= width {
            if !current.is_empty() {
                current.push(' ');
                current_source.push(' ');
            }
            current.push_str(&word);
            current_source.push_str(&word);
        } else {
            if !current.is_empty() {
                lines.push((current, current_source));
            }
            if visible_width(&word) > width {
                current = truncate_to_width(&word, width);
                current_source = word;
            } else {
                current_source = word.clone();
                current = word;
            }
        }
    }
    if !current.is_empty() {
        lines.push((current, current_source));
    }
    let mut continuation = String::new();
    lines
        .into_iter()
        .map(|(line, source)| {
            let line = if continuation.is_empty() {
                line
            } else {
                format!("{continuation}{line}")
            };
            continuation = ansi_continuation(&format!("{continuation}{source}"));
            line
        })
        .collect()
}

fn is_list_marker(word: &str) -> bool {
    visible_width(word) == 1 && word.contains('-')
}

pub(super) fn compact_first_cell(cells: &mut [String]) {
    if let Some(first) = cells.first_mut()
        && let Some(compact) = compact_date_cell(first)
    {
        *first = compact;
    }
}

pub(super) fn compact_date_cell(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    if bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        Some(format!("{}\n{}", &value[..4], &value[5..]))
    } else {
        None
    }
}
