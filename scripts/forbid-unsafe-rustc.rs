use std::{
    env,
    ffi::OsString,
    process::{Command, exit},
};

const EXEMPT_CRATE: &str = "ccusage_zstd";

fn main() {
    let mut args = env::args_os().skip(1);
    let Some(rustc) = args.next() else {
        eprintln!("forbid-unsafe-rustc: cargo passes the rustc path first");
        exit(2);
    };
    let args: Vec<OsString> = args.collect();
    let exempt = args
        .windows(2)
        .any(|pair| pair[0] == "--crate-name" && pair[1] == EXEMPT_CRATE);
    let mut command = Command::new(rustc);
    command.args(&args);
    if !exempt {
        command.args(["-F", "unsafe_code"]);
    }
    match command.status() {
        Ok(status) => exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("forbid-unsafe-rustc: run rustc: {error}");
            exit(2);
        }
    }
}
