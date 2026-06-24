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
    /// Stage this macro was recorded into (`record --into`); `normalize` splices
    /// the captured flow into that stage unless `--stage` overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
}

/// One captured event, tagged by `kind`, timestamped from recording start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RawEvent {
    /// Bytes the user typed (may contain control codes like `\u{7f}`).
    Input { t_ms: u64, bytes: String },
    /// A chunk written to the PTY by the running program.
    Output { t_ms: u64, data: String },
    /// A browser scene revealed via `demo open` at this moment — `mode` is
    /// `replace` (full-canvas) or `split` (beside the terminal). `hold_ms` keeps
    /// the scene on screen at least that long; `scroll` pans the page down while
    /// it is shown.
    Open {
        t_ms: u64,
        url: String,
        mode: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hold_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        scroll: bool,
    },
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
                stage: None,
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
