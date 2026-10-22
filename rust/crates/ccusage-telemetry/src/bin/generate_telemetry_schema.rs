use std::{env, fs, process};

fn main() {
    let module = ccusage_telemetry::generate_schema_module();
    let paths = env::args().skip(1).collect::<Vec<_>>();
    if paths.is_empty() {
        print!("{module}");
        return;
    }
    for path in paths {
        if let Err(error) = fs::write(&path, &module) {
            eprintln!("failed to write {path}: {error}");
            process::exit(1);
        }
    }
}
