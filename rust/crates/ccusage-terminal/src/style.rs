use ccusage_environment::Environment;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Color {
    Blue,
    Green,
    Grey,
    Red,
    Yellow,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalStyle {
    pub color: bool,
    pub log_level: Option<u8>,
    pub no_color: bool,
}

pub fn color(style: impl Into<TerminalStyle>, value: impl AsRef<str>, color: Color) -> String {
    let style = style.into();
    let value = value.as_ref();
    if !use_color(&style) {
        return value.to_string();
    }
    let code = match color {
        Color::Blue => 34,
        Color::Green => 32,
        Color::Grey => 90,
        Color::Red => 31,
        Color::Yellow => 33,
    };
    format!("\x1b[{code}m{value}\x1b[0m")
}

fn use_color(style: &TerminalStyle) -> bool {
    let environment = Environment::current();
    if style.no_color || environment.var_os("NO_COLOR").is_some() {
        return false;
    }
    style.color || environment.var_os("FORCE_COLOR").is_some() || crate::stdout_is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_respects_injected_no_color() {
        let style = TerminalStyle {
            color: true,
            ..TerminalStyle::default()
        };

        let output =
            Environment::with_var("NO_COLOR", "").run(|| color(style, "Total", Color::Yellow));

        assert_eq!(output, "Total");
    }

    #[test]
    fn color_respects_explicit_no_color() {
        let style = TerminalStyle {
            color: true,
            log_level: None,
            no_color: true,
        };

        assert_eq!(color(style, "Total", Color::Yellow), "Total");
    }
}
