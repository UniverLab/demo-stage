//! Command-line interface definition (clap).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// `demo` — the DemoStage command-line tool.
#[derive(Debug, Parser)]
#[command(
    name = "demo",
    version,
    about = "Demos as Code — record, normalize, check and export terminal demos"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scaffold a stage (layout, panes, triggers) to record into.
    Prepare(PrepareArgs),
    /// Record an interactive session into a raw macro.
    Record(RecordArgs),
    /// Refine a raw macro into a clean, human-looking demo score.
    Normalize(NormalizeArgs),
    /// Statically validate a demo score (exit 0 = ok, 1 = invalid).
    Check(CheckArgs),
    /// Compile a demo score to a target format.
    Export(ExportArgs),
}

/// Canvas layouts a stage can be scaffolded with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Preset {
    /// One terminal pane filling the canvas.
    Single,
    /// Terminal on the left, a browser pane (e.g. a PDF) on the right.
    Split,
    /// Terminal on top, a browser pane below.
    Stacked,
}

#[derive(Debug, Args)]
pub struct PrepareArgs {
    /// Configure interactively (a guided wizard) instead of using the flags below.
    #[arg(short = 'w', long)]
    pub wizard: bool,

    /// Where to write the stage score.
    #[arg(short, long, default_value = "demo.toml")]
    pub output: PathBuf,

    /// Canvas layout to scaffold.
    #[arg(long, value_enum, default_value_t = Preset::Single)]
    pub preset: Preset,

    /// Demo name.
    #[arg(long, default_value = "demo")]
    pub name: String,

    /// A local file to show in the browser pane (turned into a `file://` URL) —
    /// e.g. the PDF the demo builds. Used by `split`/`stacked`.
    #[arg(long)]
    pub pdf: Option<PathBuf>,

    /// A URL to show in the browser pane (overrides `--pdf`).
    #[arg(long)]
    pub url: Option<String>,

    /// Canvas width in pixels.
    #[arg(long, default_value_t = 1280)]
    pub width: u32,
    /// Canvas height in pixels.
    #[arg(long, default_value_t = 720)]
    pub height: u32,
    /// Frame rate for pixel targets.
    #[arg(long, default_value_t = 15)]
    pub fps: u32,
}

#[derive(Debug, Args)]
pub struct RecordArgs {
    /// Where to write the captured raw macro.
    #[arg(short, long, default_value = "macro.raw.toml")]
    pub output: PathBuf,

    /// Auto-stop after this many milliseconds with no terminal output
    /// (0 disables — stop the recording yourself with Ctrl-D).
    #[arg(long, default_value_t = 0)]
    pub idle_timeout_ms: u64,

    /// Shell/command to run inside the capture (defaults to `$SHELL`).
    #[arg(long)]
    pub shell: Option<String>,

    /// Record into a prepared stage: `normalize` will splice the captured
    /// terminal flow into this stage's timeline instead of a fresh score.
    #[arg(long)]
    pub into: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct NormalizeArgs {
    /// The raw macro to refine.
    #[arg(default_value = "macro.raw.toml")]
    pub input: PathBuf,

    /// Where to write the normalized score.
    #[arg(short, long, default_value = "demo.toml")]
    pub output: PathBuf,

    /// Seed for the humanized typing jitter (deterministic when set).
    #[arg(long)]
    pub seed: Option<u64>,

    /// Base typing speed, milliseconds per character.
    #[arg(long, default_value_t = 80)]
    pub typing_ms: u64,

    /// Maximum jitter added per character, in milliseconds.
    #[arg(long, default_value_t = 15)]
    pub salt_ms: u64,

    /// Splice the recording into this prepared stage (keeps its layout, panes
    /// and trigger steps). Defaults to the stage stamped by `record --into`.
    #[arg(long)]
    pub stage: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    /// The demo score to validate.
    #[arg(default_value = "demo.toml")]
    pub input: PathBuf,
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Output format: cast, html, gif or mp4.
    #[arg(value_enum)]
    pub target: Target,

    /// The demo score to compile.
    #[arg(default_value = "demo.toml")]
    pub input: PathBuf,

    /// Output file or directory (target-dependent default when omitted).
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

/// Supported export targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Target {
    /// asciinema v2 cast (text only).
    Cast,
    /// Self-contained HTML player (text only).
    Html,
    /// Animated GIF (rasterized).
    Gif,
    /// MP4 video (rasterized).
    Mp4,
}
