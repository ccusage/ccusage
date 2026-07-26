use std::{env, fs, path::PathBuf};

use serde_json::Value;

#[path = "src/help_codegen.rs"]
mod help_codegen;

const CLI_HELP_JSON: &str = "src/cli-help.json";
const CLI_COMMANDS_JSON: &str = "src/cli-commands.json";

fn main() {
    println!("cargo:rerun-if-changed={CLI_HELP_JSON}");
    println!("cargo:rerun-if-changed={CLI_COMMANDS_JSON}");
    generate_cli_help_rs();
}

fn out_dir_path(file_name: &str) -> PathBuf {
    PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo")).join(file_name)
}

fn generate_cli_help_rs() {
    let option_source = fs::read_to_string(CLI_HELP_JSON).expect("read CLI help spec");
    let command_source = fs::read_to_string(CLI_COMMANDS_JSON).expect("read CLI command spec");
    let option_sets = serde_json::from_str::<Value>(&option_source).expect("parse CLI help spec");
    let command_spec =
        serde_json::from_str::<Value>(&command_source).expect("parse CLI command spec");
    let output = help_codegen::generate_cli_help_source(&option_sets, &command_spec);
    fs::write(out_dir_path("cli-help.rs"), output).expect("write generated CLI help");
}
