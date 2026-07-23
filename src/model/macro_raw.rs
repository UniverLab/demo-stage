//! The `macro.raw.toml` capture: the low-level output of `demo record`.
//!
//! Intentionally dumb — raw input bytes and PTY output chunks with millisecond
//! offsets. All the intelligence (backspace pruning, timing) lives in
//! `demo normalize`, which interprets these events.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// A raw recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawMacro {
    pub meta: RawMeta,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<RawEvent>,
}

impl RawMacro {
    pub fn load(path: &Path) -> Result<Self> {
        super::load_toml(path)
    }

    pub fn to_toml(&self) -> Result<String> {
        super::to_toml_string(self)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        super::write_toml(path, self)
    }
}

/// `[meta]` — the terminal geometry and recording parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawMeta {
    pub shell: String,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub idle_timeout_ms: u64,
    /// Target resolution chosen at capture start (width, height). When set, the
    /// normalizer sizes the layout to this instead of deriving from cols×rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<(u32, u32)>,
    /// Frame rate chosen at capture start (15/24/30). When set, the normalizer
    /// writes it into the score's `[layout]` instead of the 15 fps default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    /// Stage this macro was recorded into (`record --into`); `normalize` splices
    /// the captured flow into that stage unless `--stage` overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// `(start_ms, end_ms)` spans of meta-command activity (`demo open` and its
    /// in-session wizard) that must be excised from the finished demo — both the
    /// typed command/echo and the wizard output. Recorded live by `demo capture`;
    /// `from_raw` drops output inside them and `normalize` drops input inside them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mute_spans: Vec<(u64, u64)>,
}

impl RawMeta {
    /// Is `t_ms` inside any meta-command span (so it must not reach the demo)?
    pub fn is_muted(&self, t_ms: u64) -> bool {
        self.mute_spans
            .iter()
            .any(|(start, end)| t_ms >= *start && t_ms < *end)
    }
}

/// One captured event, tagged by `kind`, timestamped from recording start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RawEvent {
    /// Bytes the user typed (may contain control codes like `\u{7f}`).
    Input { t_ms: u64, bytes: String },
    /// A chunk written to the PTY by the running program.
    Output { t_ms: u64, data: String },
    /// A secret was entered at a detected secret prompt — only the prompt text is
    /// kept (e.g. `Vault passphrase:`), NEVER the value, so `demo record` can ask
    /// for it again (in memory) when it re-executes. See [`crate::model::Step`].
    Secret { t_ms: u64, prompt: String },
    /// The active view switches to these panes at this moment (until the next
    /// reveal, or the end of the demo). Both `demo focus` (predefined sources) and
    /// `demo open` (ad-hoc URLs) produce one. One pane fills the canvas; two are
    /// split by `orientation`. A single terminal pane means "back to the terminal".
    /// `hold_ms` keeps the view on screen at least that long; `scroll` pans any
    /// browser pane down. For an interactive `--view` session a pane's `url` is a
    /// `viewframes:<dir>` pointer to pre-recorded frames.
    Reveal {
        t_ms: u64,
        panes: Vec<RevealPane>,
        #[serde(default, skip_serializing_if = "Orientation::is_horizontal")]
        orientation: Orientation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hold_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        scroll: bool,
    },
}

/// One pane of a [`RawEvent::Reveal`]: the terminal, or a browser page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RevealPane {
    /// Display id — a source id (`main`, `docs`) or a generated name for an
    /// ad-hoc `demo open`.
    pub id: String,
    /// Browser URL (`http(s)://`, `file://`, or `viewframes:<dir>`). `None` marks
    /// the terminal pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Emulated colour scheme (`light`/`dark`) for a browser pane, or `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

impl RevealPane {
    /// A terminal pane with id `main`.
    pub fn terminal() -> Self {
        RevealPane {
            id: "main".to_string(),
            url: None,
            theme: None,
        }
    }
    /// Is this the terminal (no URL)?
    pub fn is_terminal(&self) -> bool {
        self.url.is_none()
    }
}

/// How a two-pane reveal is arranged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    /// Side by side (first left, second right).
    #[default]
    Horizontal,
    /// Stacked (first top, second bottom).
    Vertical,
}

impl Orientation {
    fn is_horizontal(&self) -> bool {
        matches!(self, Orientation::Horizontal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let original = RawMacro {
            meta: RawMeta {
                shell: "/bin/bash".into(),
                cols: 100,
                rows: 30,
                idle_timeout_ms: 3000,
                resolution: None,
                fps: None,
                stage: None,
                mute_spans: vec![(150, 320)],
            },
            events: vec![
                RawEvent::Input {
                    t_ms: 120,
                    bytes: "git".into(),
                },
                RawEvent::Output {
                    t_ms: 130,
                    data: "git".into(),
                },
                RawEvent::Input {
                    t_ms: 400,
                    bytes: "\r".into(),
                },
            ],
        };
        let rendered = original.to_toml().unwrap();
        let reparsed: RawMacro = toml::from_str(&rendered).unwrap();
        assert_eq!(original, reparsed);
    }

    #[test]
    fn is_muted_inside_span() {
        let meta = RawMeta {
            mute_spans: vec![(100, 200), (500, 600)],
            ..default_meta()
        };
        assert!(meta.is_muted(150));
        assert!(meta.is_muted(100));
        assert!(!meta.is_muted(200));
        assert!(meta.is_muted(550));
        assert!(!meta.is_muted(300));
    }

    #[test]
    fn is_muted_empty_spans() {
        let meta = RawMeta {
            mute_spans: vec![],
            ..default_meta()
        };
        assert!(!meta.is_muted(0));
        assert!(!meta.is_muted(99999));
    }

    #[test]
    fn reveal_pane_terminal_factory() {
        let p = RevealPane::terminal();
        assert_eq!(p.id, "main");
        assert!(p.url.is_none());
        assert!(p.theme.is_none());
    }

    #[test]
    fn reveal_pane_is_terminal_true_when_no_url() {
        let p = RevealPane {
            id: "x".into(),
            url: None,
            theme: None,
        };
        assert!(p.is_terminal());
    }

    #[test]
    fn reveal_pane_is_terminal_false_when_url() {
        let p = RevealPane {
            id: "x".into(),
            url: Some("http://x.com".into()),
            theme: None,
        };
        assert!(!p.is_terminal());
    }

    #[test]
    fn orientation_default_is_horizontal() {
        let o = Orientation::default();
        assert_eq!(o, Orientation::Horizontal);
    }

    #[test]
    fn orientation_is_horizontal_method() {
        assert!(Orientation::Horizontal.is_horizontal());
        assert!(!Orientation::Vertical.is_horizontal());
    }

    #[test]
    fn raw_macro_round_trip_with_resolution_and_fps() {
        let original = RawMacro {
            meta: RawMeta {
                shell: "/bin/zsh".into(),
                cols: 120,
                rows: 40,
                idle_timeout_ms: 5000,
                resolution: Some((1920, 1080)),
                fps: Some(24),
                stage: Some("intro".into()),
                mute_spans: vec![],
            },
            events: vec![RawEvent::Secret {
                t_ms: 100,
                prompt: "Vault passphrase:".into(),
            }],
        };
        let rendered = original.to_toml().unwrap();
        let reparsed: RawMacro = toml::from_str(&rendered).unwrap();
        assert_eq!(original, reparsed);
    }

    fn default_meta() -> RawMeta {
        RawMeta {
            shell: "/bin/bash".into(),
            cols: 80,
            rows: 24,
            idle_timeout_ms: 0,
            resolution: None,
            fps: None,
            stage: None,
            mute_spans: vec![],
        }
    }
}
