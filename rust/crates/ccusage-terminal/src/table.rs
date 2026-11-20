use std::io::{self, Write};

use crate::{
    style::{Color, TerminalStyle, color},
    terminal::DEFAULT_TERMINAL_WIDTH,
    width::{
        ansi_continuation, ensure_ansi_reset, truncate_to_width, visible_width,
        visible_width_max_line,
    },
};

const MAX_MODELS_CONTENT_WIDTH: usize = 25;
const MIN_WRAP_COLUMN_WIDTH: usize = 8;
const MAX_FALLBACK_TEXT_WIDTH: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Align {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColumnKind {
    Primary,
    Detail,
    Models,
    Text,
    Numeric,
}

impl ColumnKind {
    fn alignment(self) -> Align {
        match self {
            Self::Numeric => Align::Right,
            Self::Primary | Self::Detail | Self::Models | Self::Text => Align::Left,
        }
    }
}

pub struct SimpleTable {
    headers: Vec<String>,
    columns: Vec<ColumnKind>,
    rows: Vec<Option<TableRow>>,
    style: TerminalStyle,
    terminal_width: usize,
    compact_dates: bool,
    date_compaction_width: Option<usize>,
}

struct TableRow {
    cells: Vec<String>,
    first_align: Option<Align>,
}

struct ContentWidths<'a> {
    numeric: &'a [usize],
    text: &'a [usize],
}

impl SimpleTable {
    pub fn new(
        headers: Vec<&str>,
        columns: Vec<ColumnKind>,
        style: impl Into<TerminalStyle>,
    ) -> Self {
        Self {
            headers: headers.into_iter().map(str::to_string).collect(),
            columns,
            rows: Vec::new(),
            style: style.into(),
            terminal_width: DEFAULT_TERMINAL_WIDTH,
            compact_dates: false,
            date_compaction_width: None,
        }
    }

    pub fn with_terminal_width(mut self, width: usize) -> Self {
        self.terminal_width = width;
        self
    }

    pub fn with_date_compaction(mut self, compact_dates: bool) -> Self {
        self.compact_dates = compact_dates;
        self
    }

    pub fn with_date_compaction_at(mut self, terminal_width: usize) -> Self {
        self.date_compaction_width = Some(terminal_width);
        self
    }

    pub fn push(&mut self, row: Vec<String>) {
        self.push_row(row, None);
    }

    pub fn push_aligned(&mut self, row: Vec<String>, first_align: Align) {
        self.push_row(row, Some(first_align));
    }

    fn push_row(&mut self, mut cells: Vec<String>, first_align: Option<Align>) {
        if self
            .date_compaction_width
            .is_some_and(|width| self.terminal_width <= width)
        {
            compact_first_cell(&mut cells);
        }
        self.rows.push(Some(TableRow { cells, first_align }));
    }

    pub fn separator(&mut self) {
        self.rows.push(None);
    }

    pub fn column_count(&self) -> usize {
        self.headers.len()
    }

    pub fn print(&self) -> io::Result<()> {
        let mut stdout = crate::stdout();
        for line in self.render_lines() {
            writeln!(stdout, "{line}")?;
        }
        Ok(())
    }

    fn render_lines(&self) -> Vec<String> {
        let widths = self.column_widths();
        let mut lines = Vec::new();
        lines.push(border('┌', '┬', '┐', &widths));
        for header_row in expand_multiline_row(&self.headers, self.headers.len(), &widths) {
            let header_row = header_row
                .iter()
                .map(|header| color(self.style, header, Color::Blue))
                .collect::<Vec<_>>();
            lines.push(table_line(&header_row, &self.columns, &widths, None));
        }
        lines.push(border('├', '┼', '┤', &widths));
        for (row_index, row) in self.rows.iter().enumerate() {
            match row {
                Some(row) => {
                    let cells = self.compact_date_row(&row.cells, &widths);
                    for physical_row in expand_multiline_row(&cells, self.headers.len(), &widths) {
                        lines.push(table_line(
                            &physical_row,
                            &self.columns,
                            &widths,
                            row.first_align,
                        ));
                    }
                }
                None => lines.push(border('├', '┼', '┤', &widths)),
            }
            if row.is_some()
                && row_index + 1 < self.rows.len()
                && !matches!(self.rows.get(row_index + 1), Some(None))
            {
                lines.push(border('├', '┼', '┤', &widths));
            }
        }
        lines.push(border('└', '┴', '┘', &widths));
        lines
    }

    fn column_widths(&self) -> Vec<usize> {
        let model_column = self
            .columns
            .iter()
            .position(|role| *role == ColumnKind::Models);
        let numeric_content_widths = self
            .columns
            .iter()
            .enumerate()
            .map(|(index, role)| {
                if *role != ColumnKind::Numeric {
                    return 0;
                }
                self.rows
                    .iter()
                    .flatten()
                    .filter_map(|row| row.cells.get(index))
                    .map(|cell| visible_width_max_line(cell))
                    .max()
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        let text_content_widths = self
            .columns
            .iter()
            .enumerate()
            .map(|(index, role)| {
                if *role == ColumnKind::Numeric {
                    return 0;
                }
                self.rows
                    .iter()
                    .flatten()
                    .filter_map(|row| row.cells.get(index))
                    .map(|cell| visible_width_max_line(cell))
                    .max()
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        let content_widths = self
            .headers
            .iter()
            .enumerate()
            .map(|(index, header)| {
                let width = visible_width_max_line(header);
                if model_column == Some(index) {
                    width.min(MAX_MODELS_CONTENT_WIDTH)
                } else {
                    width
                }
            })
            .collect::<Vec<_>>();
        let mut content_widths = content_widths;
        for row in self.rows.iter().flatten() {
            for (index, cell) in row.cells.iter().enumerate() {
                let cell_width = visible_width_max_line(cell);
                let cell_width = if model_column == Some(index) {
                    cell_width.min(MAX_MODELS_CONTENT_WIDTH)
                } else {
                    cell_width
                };
                if let Some(width) = content_widths.get_mut(index) {
                    *width = (*width).max(cell_width);
                }
            }
        }
        let widths = content_widths
            .iter()
            .enumerate()
            .map(|(index, width)| {
                if model_column == Some(index) {
                    (width + 2).clamp(15, MAX_MODELS_CONTENT_WIDTH + 2)
                } else if self.columns.get(index) == Some(&ColumnKind::Numeric) {
                    (width + 3).max(11)
                } else if self.columns.get(index) == Some(&ColumnKind::Detail) {
                    (width + 2).max(15)
                } else {
                    (width + 2).max(10)
                }
            })
            .collect::<Vec<_>>();
        let total_required = cli_table_required_width(&widths);
        let first_column_min = if self.compact_dates && total_required <= self.terminal_width {
            12
        } else {
            10
        };
        fit_widths_to_terminal(
            widths,
            &self.columns,
            self.terminal_width,
            first_column_min,
            model_column.is_some(),
            ContentWidths {
                numeric: &numeric_content_widths,
                text: &text_content_widths,
            },
            self.compact_dates,
        )
    }

    fn compact_date_row(&self, row: &[String], widths: &[usize]) -> Vec<String> {
        if !self.compact_dates
            || widths
                .first()
                .copied()
                .unwrap_or_default()
                .saturating_sub(2)
                >= 10
        {
            return row.to_vec();
        }
        let mut row = row.to_vec();
        compact_first_cell(&mut row);
        row
    }
}

fn expand_multiline_row(row: &[String], column_count: usize, widths: &[usize]) -> Vec<Vec<String>> {
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

fn fit_widths_to_terminal(
    mut widths: Vec<usize>,
    columns: &[ColumnKind],
    terminal_width: usize,
    first_column_min: usize,
    has_model_column: bool,
    content_widths: ContentWidths<'_>,
    compact_dates: bool,
) -> Vec<usize> {
    if cli_table_required_width(&widths) <= terminal_width {
        return widths;
    }

    let fallback_minimums = widths
        .iter()
        .enumerate()
        .map(|(index, _)| {
            if columns.get(index) == Some(&ColumnKind::Numeric) {
                10
            } else if columns.get(index) == Some(&ColumnKind::Primary) {
                first_column_min
            } else if preferred_wrap_column(columns.get(index).copied(), has_model_column) {
                12
            } else {
                content_widths
                    .text
                    .get(index)
                    .copied()
                    .unwrap_or_default()
                    .saturating_add(2)
                    .clamp(MIN_WRAP_COLUMN_WIDTH, MAX_FALLBACK_TEXT_WIDTH)
            }
        })
        .collect::<Vec<_>>();

    let mut content_minimums = fallback_minimums
        .iter()
        .enumerate()
        .map(|(index, minimum)| {
            if columns.get(index) == Some(&ColumnKind::Numeric) {
                (*minimum).max(
                    content_widths
                        .numeric
                        .get(index)
                        .copied()
                        .unwrap_or_default()
                        .saturating_add(2),
                )
            } else {
                *minimum
            }
        })
        .collect::<Vec<_>>();
    while cli_table_required_width(&content_minimums) > terminal_width {
        let Some(index) = content_minimums
            .iter()
            .enumerate()
            .filter(|(index, width)| {
                columns.get(*index) != Some(&ColumnKind::Numeric) && **width > MIN_WRAP_COLUMN_WIDTH
            })
            .max_by_key(|(index, width)| {
                (
                    preferred_wrap_column(columns.get(*index).copied(), has_model_column),
                    **width,
                )
            })
            .map(|(index, _)| index)
        else {
            break;
        };
        content_minimums[index] -= 1;
    }
    let natural_widths = widths.clone();
    let mut expansion_limits = natural_widths.clone();
    if compact_dates && let Some(first) = expansion_limits.first_mut() {
        *first = first_column_min;
    }
    if cli_table_required_width(&content_minimums) <= terminal_width {
        let mut widths = content_minimums;
        let spare = terminal_width.saturating_sub(cli_table_required_width(&widths));
        distribute_spare_width(&mut widths, &expansion_limits, spare);
        return widths;
    }

    let minimums = fallback_minimums;
    let available_width = terminal_width.saturating_sub(widths.len() + 1);
    let total_content_width = widths.iter().sum::<usize>();
    if total_content_width > 0 {
        let scale = available_width as f64 / total_content_width as f64;
        for (index, width) in widths.iter_mut().enumerate() {
            let scaled = (*width as f64 * scale).floor() as usize;
            *width = scaled.max(minimums[index]);
        }
    }

    while cli_table_required_width(&widths) > terminal_width {
        let Some(index) = widths
            .iter()
            .enumerate()
            .filter(|(index, width)| **width > minimums[*index])
            .max_by_key(|(index, width)| {
                (columns.get(*index) != Some(&ColumnKind::Numeric), **width)
            })
            .map(|(index, _)| index)
        else {
            break;
        };
        widths[index] -= 1;
    }

    let spare = terminal_width.saturating_sub(cli_table_required_width(&widths));
    distribute_spare_width(&mut widths, &expansion_limits, spare);
    widths
}

fn preferred_wrap_column(kind: Option<ColumnKind>, has_model_column: bool) -> bool {
    match kind {
        Some(ColumnKind::Models) => true,
        Some(ColumnKind::Detail) => !has_model_column,
        _ => false,
    }
}

fn distribute_spare_width(widths: &mut [usize], natural_widths: &[usize], mut spare: usize) {
    if widths.is_empty() {
        return;
    }
    let mut start = 0;
    while spare > 0 {
        let Some(index) = (0..widths.len())
            .map(|offset| (start + offset) % widths.len())
            .find(|index| widths[*index] < natural_widths[*index])
        else {
            break;
        };
        widths[index] += 1;
        spare -= 1;
        start = (index + 1) % widths.len();
    }
}

fn cli_table_required_width(widths: &[usize]) -> usize {
    widths.iter().sum::<usize>() + widths.len() + 1
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

fn compact_first_cell(cells: &mut [String]) {
    if let Some(first) = cells.first_mut()
        && let Some(compact) = compact_date_cell(first)
    {
        *first = compact;
    }
}

fn compact_date_cell(value: &str) -> Option<String> {
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

fn table_line(
    cells: &[String],
    columns: &[ColumnKind],
    widths: &[usize],
    first_align: Option<Align>,
) -> String {
    let mut line = String::from("│");
    for (index, width) in widths.iter().enumerate() {
        let cell = cells.get(index).map(String::as_str).unwrap_or("");
        let align = first_align
            .filter(|_| index == 0)
            .or_else(|| columns.get(index).copied().map(ColumnKind::alignment))
            .unwrap_or(Align::Left);
        line.push(' ');
        line.push_str(&pad_cell(cell, width.saturating_sub(2), align));
        line.push(' ');
        line.push('│');
    }
    line
}

fn pad_cell(cell: &str, width: usize, align: Align) -> String {
    let cell = ensure_ansi_reset(cell);
    let visible = visible_width(&cell);
    if visible >= width {
        return cell;
    }
    let padding = width - visible;
    match align {
        Align::Left => format!("{cell}{}", " ".repeat(padding)),
        Align::Right => format!("{}{cell}", " ".repeat(padding)),
    }
}

fn border(left: char, middle: char, right: char, widths: &[usize]) -> String {
    let mut line = String::new();
    line.push(left);
    for (index, width) in widths.iter().enumerate() {
        line.push_str(&"─".repeat(*width));
        line.push(if index + 1 == widths.len() {
            right
        } else {
            middle
        });
    }
    line
}

#[cfg(test)]
mod tests;
