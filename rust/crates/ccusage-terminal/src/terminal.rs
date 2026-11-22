use ccusage_environment::Environment;

pub(crate) const DEFAULT_TERMINAL_WIDTH: usize = 120;

pub fn terminal_width() -> usize {
    select_terminal_width(
        Environment::current().var("COLUMNS").ok().as_deref(),
        terminal_size_width(),
    )
}

fn terminal_size_width() -> Option<usize> {
    terminal_size::terminal_size().map(|(terminal_size::Width(width), _)| width as usize)
}

fn select_terminal_width(columns: Option<&str>, detected_width: Option<usize>) -> usize {
    columns
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|width| *width > 0)
        .or_else(|| detected_width.filter(|width| *width > 0))
        .unwrap_or(DEFAULT_TERMINAL_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_columns_before_detected_width() {
        assert_eq!(select_terminal_width(Some("80"), Some(100)), 80);
    }

    #[test]
    fn ignores_invalid_columns() {
        assert_eq!(select_terminal_width(Some("wide"), Some(100)), 100);
    }

    #[test]
    fn uses_default_width_when_no_width_is_available() {
        assert_eq!(select_terminal_width(None, None), DEFAULT_TERMINAL_WIDTH);
    }

    #[test]
    fn ignores_zero_detected_width() {
        assert_eq!(select_terminal_width(None, Some(0)), DEFAULT_TERMINAL_WIDTH);
    }
}
