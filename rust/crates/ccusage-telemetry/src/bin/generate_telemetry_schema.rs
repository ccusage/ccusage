use std::{env, fs, io};

fn main() -> io::Result<()> {
    let module = ccusage_telemetry::generate_schema_module();
    let paths = env::args().skip(1).collect::<Vec<_>>();
    if paths.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected at least one output path",
        ));
    }
    for path in paths {
        fs::write(&path, &module).map_err(|error| {
            io::Error::new(error.kind(), format!("failed to write {path}: {error}"))
        })?;
    }
    Ok(())
}
