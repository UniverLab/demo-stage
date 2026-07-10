use std::process::ExitCode;

use clap::Parser;
use demo_stage::cli::Cli;

fn main() -> ExitCode {
    match demo_stage::run(Cli::parse()) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
