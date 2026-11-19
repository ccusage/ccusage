use std::{env, fs, process::ExitCode};

fn main() -> ExitCode {
    let schema = match ccusage_config::generate_config_schema_json() {
        Ok(schema) => schema,
        Err(error) => {
            eprintln!("failed to generate config schema: {error}");
            return ExitCode::FAILURE;
        }
    };
    let paths = env::args().skip(1).collect::<Vec<_>>();
    if paths.is_empty() {
        print!("{schema}");
        return ExitCode::SUCCESS;
    }
    for path in paths {
        if let Err(error) = fs::write(&path, &schema) {
            eprintln!("failed to write {path}: {error}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
