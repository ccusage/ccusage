use std::{env, fs, process};

fn main() {
    let schema = ccusage_config::generate_config_schema_json();
    let paths = env::args().skip(1).collect::<Vec<_>>();
    if paths.is_empty() {
        print!("{schema}");
        return;
    }
    for path in paths {
        if let Err(error) = fs::write(&path, &schema) {
            eprintln!("failed to write {path}: {error}");
            process::exit(1);
        }
    }
}
