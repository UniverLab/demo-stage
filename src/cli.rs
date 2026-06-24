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
    /// Capture a live interactive session, then normalize it to a demo score.
    Capture(CaptureArgs),
    /// Reveal a browser scene in the running capture (from here or another shell).
    Open(OpenArgs),
    /// End the in-progress capture — run this inside a `demo capture` session.
    Stop,
    /// Execute a demo score in a PTY to (re)produce a recording (a .rec).
    Record(RecordArgs),
    /// Render a recording to one or more formats (playback — never executes).
    Export(ExportArgs),
}

/// How a `demo open` browser scene sits on the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OpenMode {
    /// Full-canvas: the browser takes over the whole frame (a scene swap).
    Replace,
    /// Beside the terminal (terminal keeps showing, browser to the right).
    Split,
}

#[derive(Debug, Args)]
pub struct OpenArgs {
    /// URL to show (e.g. a repo page, a `file://` PDF, a localhost server).
    /// Omit it (on a terminal) to be prompted by a small wizard.
    pub url: Option<String>,

    /// Reveal full-canvas (`replace`, the default) or beside the terminal (`split`).
    #[arg(long, value_enum, default_value_t = OpenMode::Replace)]
    pub mode: OpenMode,

    /// Shortcut for `--mode split`.
    #[arg(long, conflicts_with = "mode")]
    pub split: bool,

    /// Defer the reveal until this substring appears in the terminal output —
    /// arm it before running the program, so the scene opens on a cue line.
    #[arg(long)]
    pub when: Option<String>,

    /// Force the interactive wizard even if a URL/flags are given.
    #[arg(short = 'w', long)]
    pub wizard: bool,
}

#[derive(Debug, Args)]
pub struct CaptureArgs {
    /// Where to write the captured raw macro.
    #[arg(short, long, default_value = "macro.raw.toml")]
    pub output: PathBuf,

    /// Auto-stop after this many milliseconds with no terminal output
    /// (0 disables — stop the capture yourself with `demo stop`).
    #[arg(long, default_value_t = 0)]
    pub idle_timeout_ms: u64,

    /// Shell/command to run inside the capture (defaults to `$SHELL`).
    #[arg(long)]
    pub shell: Option<String>,

    /// Capture into a prepared stage: normalize will splice the captured
    /// terminal flow into this stage's timeline instead of a fresh score.
    #[arg(long)]
    pub into: Option<PathBuf>,

    /// Skip the automatic normalize pass — keep only the raw macro.
    #[arg(long)]
    pub no_normalize: bool,

    /// Write a timestamped diagnostic log of every input/output chunk (with hex)
    /// next to the raw macro (`<output>.debug.log`), for debugging captures.
    #[arg(long)]
    pub debug: bool,

    /// Where the automatic normalize writes the demo score.
    #[arg(short = 'O', long, default_value = "demo.toml")]
    pub normalized_output: PathBuf,

    /// Force a clean PS1 in the captured shell (default: the built-in realistic
    /// prompt), so the demo shows a tidy prompt instead of your real one. Pass a
    /// value to customize.
    #[arg(long, value_name = "PS1")]
    pub prompt: Option<String>,

    /// Keep your shell's real prompt during capture (don't force a clean one).
    #[arg(long, conflicts_with = "prompt")]
    pub keep_prompt: bool,
}

#[derive(Debug, Args)]
pub struct RecordArgs {
    /// The demo score to execute.
    #[arg(default_value = "demo.toml")]
    pub input: PathBuf,

    /// Where to write the recording (a `.rec` that `export` plays back).
    #[arg(short, long, default_value = "demo.rec")]
    pub output: PathBuf,
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
pub struct ExportArgs {
    /// Formats to build, comma-separated — `gif`, `mp4`, or `all` (e.g. `gif,mp4`).
    /// Omit it to build every supported format.
    #[arg(value_parser = parse_targets)]
    pub targets: Option<TargetList>,

    /// The recording to render: a `.rec` from `demo record`, or a raw capture
    /// (`macro.raw.toml`) to render the live session directly.
    #[arg(default_value = "demo.rec")]
    pub input: PathBuf,

    /// Speed multiplier applied to typing and waits — e.g. `2x`, `3x`, `0.5x`
    /// (a bare number works too). `1x` keeps the recorded pace.
    #[arg(long, default_value = "1x", value_parser = parse_speed)]
    pub speed: f64,
}

/// One or more export targets parsed from a comma-separated token.
#[derive(Debug, Clone)]
pub struct TargetList(pub Vec<Target>);

/// Every format `demo export` builds when no target is given.
pub fn all_targets() -> Vec<Target> {
    vec![Target::Gif, Target::Mp4]
}

/// Parse `gif,mp4` (or `all`) into a deduplicated list of targets.
fn parse_targets(s: &str) -> Result<TargetList, String> {
    let mut out: Vec<Target> = Vec::new();
    for part in s.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if p.eq_ignore_ascii_case("all") {
            return Ok(TargetList(all_targets()));
        }
        let t = <Target as ValueEnum>::from_str(p, true)
            .map_err(|_| format!("invalid format '{p}' (expected gif, mp4 or all)"))?;
        if !out.contains(&t) {
            out.push(t);
        }
    }
    if out.is_empty() {
        return Err("no export formats given (try gif, mp4 or all)".to_string());
    }
    Ok(TargetList(out))
}

/// Parse a speed multiplier like `2x`, `3x`, `0.5x` or a bare `2`.
fn parse_speed(s: &str) -> Result<f64, String> {
    let trimmed = s.trim();
    let value = trimmed.strip_suffix(['x', 'X']).unwrap_or(trimmed);
    let v: f64 = value
        .parse()
        .map_err(|_| format!("invalid speed '{s}' (try 2x, 3x or 0.5x)"))?;
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(format!("speed must be a positive number (got '{s}')"))
    }
}

/// Supported export targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Target {
    /// Animated GIF (rasterized) — for READMEs, chat, anywhere `<img>` works.
    Gif,
    /// MP4 video (rasterized) — for landings and the web (`<video>`).
    Mp4,
}
