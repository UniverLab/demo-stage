//! DemoStage — Demos as Code.
//!
//! Records a terminal session as a sequence of events (a *macro*), normalizes
//! human imperfections into a clean *score* (`demo.toml`), and compiles that
//! score to several output formats. See `SPEC-0002` in the workspace root.

pub mod cli;
pub mod commands;
pub mod error;
pub mod export;
pub mod fonts;
pub mod model;
pub mod normalize;
pub mod validate;

pub use error::{Error, Result};

/// The command a user types inside a capture to end it (see `demo stop`). The
/// normalizer drops it from the score so it never shows up in the finished demo.
pub const STOP_COMMAND: &str = "demo stop";

pub const BANNER: &str = "\
     █████                                    █████████   █████\n\
    ░░███                                    ███░░░░░███ ░░███\n\
  ███████   ██████  █████████████    ██████ ░███    ░░░  ███████    ██████    ███████  ██████\n\
 ███░░███  ███░░███░░███░░███░░███  ███░░███░░█████████ ░░░███░    ░░░░░███  ███░░███ ███░░███\n\
░███ ░███ ░███████  ░███ ░███ ░███ ░███ ░███ ░░░░░░░░███  ░███      ███████ ░███ ░███░███████\n\
░███ ░███ ░███░░░   ░███ ░███ ░███ ░███ ░███ ███    ░███  ░███ ███ ███░░███ ░███ ░███░███░░░\n\
░░████████░░██████  █████░███ █████░░██████ ░░█████████   ░░█████ ░░████████░░███████░░██████\n\
 ░░░░░░░░  ░░░░░░  ░░░░░ ░░░ ░░░░░  ░░░░░░   ░░░░░░░░░     ░░░░░   ░░░░░░░░  ░░░░░███ ░░░░░░\n\
                                                                              ███ ░███\n\
                                                                             ░░██████\n\
                                                                              ░░░░░░";

use std::process::ExitCode;

use cli::{Cli, Command};

/// Dispatch a parsed CLI invocation to its command, returning the process exit
/// code. Only `check` reports failure through the exit code; everything else
/// surfaces problems as an [`Error`].
pub fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Command::Capture(args) => commands::capture::run(args).map(|()| ExitCode::SUCCESS),
        Command::Open(args) => commands::open::run(args).map(|()| ExitCode::SUCCESS),
        Command::Stop => commands::stop::run().map(|()| ExitCode::SUCCESS),
        Command::Record(args) => commands::record::run(args).map(|()| ExitCode::SUCCESS),
        Command::Export(args) => commands::export::run(args).map(|()| ExitCode::SUCCESS),
        Command::Doctor(args) => commands::doctor::run(args).map(|()| ExitCode::SUCCESS),
        Command::Edit(args) => commands::edit::run(args).map(|()| ExitCode::SUCCESS),
        Command::Source(args) => commands::source::run(args).map(|()| ExitCode::SUCCESS),
        Command::Scene(args) => commands::scene::run(args).map(|()| ExitCode::SUCCESS),
        Command::Focus(args) => commands::focus::run(args).map(|()| ExitCode::SUCCESS),
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        crate::cli::Cli::command().debug_assert();
    }
}
