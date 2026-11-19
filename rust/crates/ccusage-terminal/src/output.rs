use std::{
    fmt,
    io::{self, IsTerminal, StdoutLock, Write},
};

pub fn stdout() -> StdoutLock<'static> {
    io::stdout().lock()
}

pub fn stdout_is_terminal() -> bool {
    io::stdout().is_terminal()
}

pub fn write_stdout(text: fmt::Arguments<'_>) -> io::Result<()> {
    io::stdout().lock().write_fmt(text)
}

pub fn write_stderr(text: fmt::Arguments<'_>) {
    let _ = io::stderr().lock().write_fmt(text);
}

#[macro_export]
macro_rules! outln {
    () => {
        $crate::write_stdout(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::write_stdout(format_args!("{}\n", format_args!($($arg)*)))
    };
}

#[macro_export]
macro_rules! errln {
    () => {
        $crate::write_stderr(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::write_stderr(format_args!("{}\n", format_args!($($arg)*)))
    };
}
