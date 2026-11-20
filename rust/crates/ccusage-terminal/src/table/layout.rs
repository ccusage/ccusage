use super::ColumnKind;

const MIN_WRAP_COLUMN_WIDTH: usize = 8;
const MAX_FALLBACK_TEXT_WIDTH: usize = 12;

pub(super) struct ContentWidths<'a> {
    pub(super) numeric: &'a [usize],
    pub(super) text: &'a [usize],
}

pub(super) fn fit_widths_to_terminal(
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

pub(super) fn cli_table_required_width(widths: &[usize]) -> usize {
    widths.iter().sum::<usize>() + widths.len() + 1
}
