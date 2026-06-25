//! The `demo.toml` score: the clean, declarative output of `demo normalize`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// A complete demo score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub demo: DemoMeta,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Env>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typing: Option<Typing>,
    pub layout: Layout,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timeline: Vec<Step>,
}

impl Score {
    /// Load and parse a score from a TOML file.
    pub fn load(path: &Path) -> Result<Self> {
        super::load_toml(path)
    }

    /// Render this score as a pretty TOML string.
    pub fn to_toml(&self) -> Result<String> {
        super::to_toml_string(self)
    }

    /// Write this score to a TOML file.
    pub fn save(&self, path: &Path) -> Result<()> {
        super::write_toml(path, self)
    }

    /// Find a pane by id.
    pub fn pane(&self, id: &str) -> Option<&Pane> {
        self.layout.panes.iter().find(|p| p.id == id)
    }
}

/// `[demo]` — top-level metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DemoMeta {
    pub name: String,
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,
    /// Shell prompt shown in the exported demo (bash `PS1` syntax, so colours via
    /// `\[\e[..m\]` and escapes like `\w` work). Absent → the built-in default, a
    /// realistic generic Linux prompt (`user@demo:~$`, green/blue). Set it to e.g.
    /// `"$ "` for a bare prompt or `"\[\e[32m\]❯\[\e[0m\] "` for a green arrow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

fn default_output_dir() -> PathBuf {
    PathBuf::from("./dist")
}

/// `[env]` — optional sandbox setup/teardown for an isolated run.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Env {
    #[serde(default)]
    pub isolated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_script: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teardown_script: Option<String>,
    /// Environment variables the export run needs (e.g. a token that lets a flow
    /// skip a secret prompt). Names only — values come from the runner's
    /// environment, never the score. `check` reports any that are unset.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
}

/// `[typing]` — humanized-typing parameters consumed at export time. Set by
/// `demo normalize`; controls the per-character jitter for `human_salt` steps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Typing {
    /// Base speed, milliseconds per character.
    #[serde(default = "default_base_ms")]
    pub base_ms: u64,
    /// Maximum jitter added/removed per character, in milliseconds.
    #[serde(default = "default_salt_ms")]
    pub salt_ms: u64,
    /// Seed for reproducible jitter (random each run when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

fn default_base_ms() -> u64 {
    80
}

fn default_salt_ms() -> u64 {
    15
}

impl Default for Typing {
    fn default() -> Self {
        Typing {
            base_ms: default_base_ms(),
            salt_ms: default_salt_ms(),
            seed: None,
        }
    }
}

/// `[layout]` — the global canvas and its panes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_fps")]
    pub fps: u32,
    /// Line height as a multiple of the font size on the pixel targets (gif/mp4).
    /// Defaults to `1.2` (room for descenders like `j p q g`). Box-drawing and
    /// block glyphs are drawn to fill the cell, so they stay solid at any value.
    #[serde(default = "default_line_height")]
    pub line_height: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panes: Vec<Pane>,
}

fn default_fps() -> u32 {
    15
}

fn default_line_height() -> f32 {
    1.2
}

/// `[[layout.panes]]` — one scene placed on the canvas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pane {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: PaneKind,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    // terminal-only
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<u32>,
    // browser-only. For an interactive `--view` scene this is a `viewframes:<dir>`
    // pointer to pre-recorded frames; `export` plays those back instead of
    // capturing the page via headless Chromium (see [`super::view_frames_dir`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// The kind of renderer backing a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneKind {
    Terminal,
    Browser,
}

/// One step on the shared timeline, tagged by its `action`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Step {
    /// Make a pane the active target for subsequent input.
    Focus { pane: String },
    /// Type text into the focused terminal pane.
    Type {
        text: String,
        #[serde(default, skip_serializing_if = "is_false")]
        human_salt: bool,
    },
    /// Press a single named key (e.g. `enter`, `tab`, `ctrl+c`).
    Keypress { key: String },
    /// Block until a substring appears in a pane's output.
    WaitForStdout {
        #[serde(rename = "match")]
        pattern: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<String>,
    },
    /// Hold for a fixed duration.
    Wait { duration_ms: u64 },
    /// Show an on-canvas caption (a step indicator) until the next caption;
    /// an empty `text` clears it. Rendered on pixel targets (gif/mp4); ignored
    /// by the text-only `cast`/`html`.
    Caption { text: String },
    /// Scroll a (browser) pane.
    Scroll {
        direction: ScrollDirection,
        #[serde(default)]
        velocity: Velocity,
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<String>,
    },
    /// End the demo.
    Terminate,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Velocity {
    #[default]
    Constant,
}

#[cfg(test)]
mod tests {
    use super::*;

    // The exact example from SPEC-0002 §5 must parse into our model.
    const SPEC_EXAMPLE: &str = r##"
[demo]
name = "univerlab-agent-experiment"
output_dir = "./dist"

[env]
isolated = true
setup_script = "mkdir -p /tmp/demo-sandbox && cd /tmp/demo-sandbox"
teardown_script = "rm -rf /tmp/demo-sandbox"

[layout]
width = 1920
height = 1080
fps = 15
background = "#0b0f14"

  [[layout.panes]]
  id = "console"
  type = "terminal"
  x = 0
  y = 0
  width = 960
  height = 1080
  font_family = "JetBrainsMono Nerd Font"
  font_size = 16

  [[layout.panes]]
  id = "preview"
  type = "browser"
  x = 960
  y = 0
  width = 960
  height = 1080
  url = "file:///tmp/demo-sandbox/output.pdf"

[[timeline]]
action = "focus"
pane = "console"

[[timeline]]
action = "type"
text = "opencode --agent --generate-report"
human_salt = true

[[timeline]]
action = "keypress"
key = "enter"

[[timeline]]
action = "wait_for_stdout"
match = "Report generated successfully."
pane = "console"

[[timeline]]
action = "focus"
pane = "preview"

[[timeline]]
action = "scroll"
direction = "down"
velocity = "constant"
duration_ms = 4000

[[timeline]]
action = "terminate"
"##;

    #[test]
    fn parses_the_spec_example() {
        let score: Score = toml::from_str(SPEC_EXAMPLE).expect("spec example parses");
        assert_eq!(score.demo.name, "univerlab-agent-experiment");
        assert_eq!(score.layout.panes.len(), 2);
        assert_eq!(score.pane("console").unwrap().kind, PaneKind::Terminal);
        assert_eq!(score.pane("preview").unwrap().kind, PaneKind::Browser);
        assert_eq!(score.timeline.len(), 7);
        assert!(matches!(score.timeline[0], Step::Focus { .. }));
        assert!(matches!(
            score.timeline[1],
            Step::Type {
                human_salt: true,
                ..
            }
        ));
        assert!(matches!(score.timeline.last(), Some(Step::Terminate)));
    }

    #[test]
    fn round_trips_through_toml() {
        let original: Score = toml::from_str(SPEC_EXAMPLE).unwrap();
        let rendered = original.to_toml().unwrap();
        let reparsed: Score = toml::from_str(&rendered).unwrap();
        assert_eq!(original, reparsed);
    }

    #[test]
    fn typing_has_sane_defaults() {
        let t = Typing::default();
        assert_eq!(t.base_ms, 80);
        assert_eq!(t.salt_ms, 15);
        assert!(t.seed.is_none());
    }
}
