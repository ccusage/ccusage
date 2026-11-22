use std::io::{self, Write};

use crate::{
    style::{Color, TerminalStyle, color},
    terminal::DEFAULT_TERMINAL_WIDTH,
    width::{ensure_ansi_reset, visible_width, visible_width_max_line},
};

const MAX_MODELS_CONTENT_WIDTH: usize = 25;

mod layout;
mod wrap;

use layout::{ContentWidths, cli_table_required_width, fit_widths_to_terminal};
use wrap::{compact_first_cell, expand_multiline_row};

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
