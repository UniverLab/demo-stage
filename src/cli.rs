//! Command-line interface definition (clap).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// `demo` — the DemoStage command-line tool.
#[derive(Debug, Parser)]
#[command(
    name = "demo",
    version,
    about = "Demos as Code — capture, record and export terminal demos"
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
    /// Check the environment for browser/video dependencies and report fixes.
    Doctor(DoctorArgs),
    /// Interactively edit timing/wait steps in a demo score.
    Edit(EditArgs),
    /// Define a content source (terminal, browser) for scene composition.
    Source(SourceArgs),
    /// Define a scene composition from pre-defined sources.
    Scene(SceneArgs),
    /// Switch focus to a scene or pane during capture.
    Focus(FocusArgs),
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Try to install what's missing (Linux/apt: a non-snap Google Chrome).
    /// Without it, `doctor` only reports and prints the commands to run.
    #[arg(long)]
    pub fix: bool,
}

#[derive(Debug, Args)]
pub struct EditArgs {
    /// The demo score to edit interactively.
    #[arg(default_value = "demo.toml")]
    pub input: PathBuf,
}

#[derive(Debug, Args)]
pub struct SourceArgs {
    /// Source ID (e.g. "main", "google"). Prompted if omitted.
    pub id: Option<String>,

    /// Source type: `terminal` or `browser`. Prompted if omitted.
    #[arg(short = 't', long, value_enum)]
    pub r#type: Option<SourceKindArg>,

    /// URL for browser sources (http, https, file://). Prompted if omitted
    /// for browser sources.
    #[arg(short = 'u', long)]
    pub url: Option<String>,

    /// Colour scheme for browser sources (`light`/`dark`). Prompted if omitted.
    #[arg(short = 'c', long, value_enum)]
    pub theme: Option<ColorScheme>,

    /// List existing sources and exit.
    #[arg(long)]
    pub list: bool,

    /// Remove a source by ID and exit.
    #[arg(long, value_name = "ID")]
    pub remove: Option<String>,

    /// The demo score file to modify.
    #[arg(short, long, default_value = "demo.toml")]
    pub score: PathBuf,
}

/// Source type for `demo source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SourceKindArg {
    Terminal,
    Browser,
}

#[derive(Debug, Args)]
pub struct SceneArgs {
    /// Scene ID (e.g. "solo", "split"). Prompted if omitted.
    pub id: Option<String>,

    /// Layout string (e.g. "main", "main+google"). Prompted if omitted.
    #[arg(short = 'l', long)]
    pub layout: Option<String>,

    /// List existing scenes and exit.
    #[arg(long)]
    pub list: bool,

    /// Remove a scene by ID and exit.
    #[arg(long, value_name = "ID")]
    pub remove: Option<String>,

    /// The demo score file to modify.
    #[arg(short, long, default_value = "demo.toml")]
    pub score: PathBuf,
}

#[derive(Debug, Args)]
pub struct FocusArgs {
    /// Scene ID to focus. Omit for an interactive picker.
    pub scene: Option<String>,

    /// Trigger: focus when this substring appears in terminal output.
    #[arg(long)]
    pub when: Option<String>,

    /// Trigger: focus after the current command finishes.
    #[arg(long)]
    pub after: bool,

    /// Trigger: focus after this many milliseconds from capture start.
    #[arg(long, value_name = "MS")]
    pub after_ms: Option<u64>,

    /// Hold focus for this duration in milliseconds (0 = until next focus).
    #[arg(long, value_name = "MS")]
    pub hold: Option<u64>,

    /// The demo score file to modify.
    #[arg(short, long, default_value = "demo.toml")]
    pub score: PathBuf,
}

/// How a `demo open` browser scene sits on the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OpenMode {
    /// Full-canvas: the browser takes over the whole frame (a scene swap).
    Replace,
    /// Beside the terminal (terminal keeps showing, browser to the right).
    Split,
}

/// Browser colour scheme to emulate for a `demo open` scene (`prefers-color-scheme`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorScheme {
    Light,
    Dark,
}

impl ColorScheme {
    /// The `prefers-color-scheme` value Chromium expects.
    pub fn as_str(self) -> &'static str {
        match self {
            ColorScheme::Light => "light",
            ColorScheme::Dark => "dark",
        }
    }
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

    /// Reveal when the current foreground command finishes — arm it, then run
    /// your command; the scene opens once output goes quiet (back at the prompt).
    #[arg(long, conflicts_with_all = ["when", "view"])]
    pub after: bool,

    /// Hold the scene on screen this long, in milliseconds, after it opens — so a
    /// reveal near the end of the capture doesn't just flash by. Mutually
    /// exclusive with `--scroll`.
    #[arg(long, value_name = "MS", conflicts_with_all = ["scroll", "view"])]
    pub hold: Option<u64>,

    /// Slowly scroll the page down while the scene is shown, instead of holding a
    /// static frame. Mutually exclusive with `--hold`.
    #[arg(long, conflicts_with_all = ["hold", "view"])]
    pub scroll: bool,

    /// Open a **real (headed) browser** you drive yourself — navigate freely; the
    /// session is recorded until you close the window, then composited into the
    /// demo. No headless Chromium is needed at export. Reveals immediately.
    #[arg(long, conflicts_with_all = ["when", "after", "scroll", "hold"])]
    pub view: bool,

    /// Emulate the browser colour scheme so theme-aware pages (GitHub, …) render
    /// `light` or `dark` instead of guessing. Omit for the page/browser default.
    #[arg(long, value_enum)]
    pub theme: Option<ColorScheme>,

    /// Force the interactive wizard even if a URL/flags are given.
    #[arg(short = 'w', long)]
    pub wizard: bool,
}

#[derive(Debug, Args)]
pub struct CaptureArgs {
    /// Where to write the recording (`.rec`) — the one artifact a capture needs,
    /// the thing `demo export` plays back. The raw macro and the editable score
    /// are optional extras (`--raw` / `--score`).
    #[arg(short = 'r', long, default_value = "demo.rec")]
    pub rec: PathBuf,

    /// Auto-stop after this many milliseconds with no terminal output
    /// (0 disables — stop the capture yourself with `demo stop`).
    #[arg(long, default_value_t = 0)]
    pub idle_timeout_ms: u64,

    /// Shell/command to run inside the capture (defaults to `$SHELL`).
    #[arg(long)]
    pub shell: Option<String>,

    /// Capture into a prepared stage: the captured terminal flow is spliced into
    /// this stage's timeline (writes the resulting score to `--score`, default
    /// `demo.toml`).
    #[arg(long)]
    pub into: Option<PathBuf>,

    /// Skip the normalize pass — don't derive a score (the recording is faithful
    /// either way).
    #[arg(long)]
    pub no_normalize: bool,

    /// Write a timestamped diagnostic log of every input/output chunk (with hex)
    /// next to the recording (`<rec>.debug.log`), for debugging captures.
    #[arg(long)]
    pub debug: bool,

    /// Also write the low-level raw capture macro here (an intermediate, for
    /// inspection/debugging). Omitted by default.
    #[arg(short = 'o', long = "raw", value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Where to write the normalized demo score — the editable "demo as code"
    /// you can re-run with `demo record`. Defaults to `demo.toml`; `--no-score`
    /// skips it if you only want the recording.
    #[arg(
        short = 'O',
        long = "score",
        default_value = "demo.toml",
        value_name = "FILE"
    )]
    pub normalized_output: PathBuf,

    /// Don't write the `demo.toml` score — leave only the recording.
    #[arg(long, conflicts_with = "normalized_output")]
    pub no_score: bool,

    /// Force a clean PS1 in the captured shell (default: the built-in realistic
    /// prompt), so the demo shows a tidy prompt instead of your real one. Pass a
    /// value to customize.
    #[arg(long, value_name = "PS1")]
    pub prompt: Option<String>,

    /// Keep your shell's real prompt during capture (don't force a clean one).
    #[arg(long, conflicts_with = "prompt")]
    pub keep_prompt: bool,

    /// Font for the exported demo (DejaVu Sans Mono, JetBrains Mono, IBM Plex
    /// Mono, Liberation Mono, Ubuntu Mono). Omit for a wizard prompt.
    #[arg(long)]
    pub font: Option<String>,
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

    /// Render a **faithful capture** as-is. By default `export` refuses one (its
    /// typing/idle aren't humanized) and points you at `demo record`; pass this to
    /// render the live capture directly anyway — needed for interactive tools and
    /// side-effecting demos that can't be re-executed.
    #[arg(long)]
    pub force: bool,
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
