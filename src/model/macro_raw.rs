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
    /// A browser scene revealed via `demo open` at this moment — `mode` is
    /// `replace` (full-canvas) or `split` (beside the terminal). `hold_ms` keeps
    /// the scene on screen at least that long; `scroll` pans the page down while
    /// it is shown. For an interactive `--view` session, `url` is a
    /// `viewframes:<dir>` pointer to pre-recorded frames (see
    /// [`crate::model::view_frames_dir`]).
    Open {
        t_ms: u64,
        url: String,
        mode: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hold_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        scroll: bool,
        /// Emulated colour scheme (`light`/`dark`), or `None` for the page default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        theme: Option<String>,
    },
    /// A focus change triggered via `/focus <scene>` during capture.
    Focus { t_ms: u64, scene: String },
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
}
