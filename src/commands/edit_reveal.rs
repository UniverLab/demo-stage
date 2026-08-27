//! Helpers for editing browser reveal steps in `demo edit` — placement (replace /
//! split), static hold vs scroll, hold duration, and the pane's file/URL.

use inquire::{Select, Text};

use crate::error::{Error, Result};
use crate::file_picker::{pick_local_file, BrowseRoots};
use crate::model::{PaneKind, Score, ScrollDirection, Step, Velocity};
use crate::paths::{file_url_absolute, repair_browser_url};

const DEFAULT_HOLD_MS: u64 = 6000;
const SCROLL_HOLD_MS: u64 = 8000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    Replace,
    SplitHorizontal,
    SplitVertical,
}

/// True when `idx` is a `focus` step targeting a browser layout pane.
pub fn is_browser_focus(score: &Score, idx: usize) -> bool {
    let Some(Step::Focus { pane: Some(id) }) = score.timeline.get(idx) else {
        return false;
    };
    score.pane(id).is_some_and(|p| p.kind == PaneKind::Browser)
}

/// Interactive editor for a browser `focus` step — opening mode and optional URL.
pub fn edit_browser_reveal(score: &mut Score, focus_idx: usize) -> Result<()> {
    let pane_id = match &score.timeline[focus_idx] {
        Step::Focus { pane: Some(id) } => id.clone(),
        _ => return Ok(()),
    };
    let pane = score
        .pane(&pane_id)
        .ok_or_else(|| Error::Export(format!("pane '{pane_id}' not found in layout")))?
        .clone();

    let placement = detect_placement(&pane, &score.layout);
    let scroll = has_scroll_after(score, focus_idx, &pane_id);
    let hold_ms = hold_after(score, focus_idx, scroll);

    println!("  reveal → {pane_id}");
    if let Some(url) = &pane.url {
        let preview: String = url.chars().take(72).collect();
        println!("  url: {preview}");
    }

    let placement_pick = ask_select(
        "Place it:",
        &[
            ("replace — full screen (scene swap)", Placement::Replace),
            ("split — beside the terminal", Placement::SplitHorizontal),
            ("split — stacked (top/bottom)", Placement::SplitVertical),
        ],
        placement,
    )?;

    let behavior = ask_select(
        "Show it as:",
        &[
            ("Static — hold for a few seconds", false),
            ("Scroll the page down (pan)", true),
        ],
        scroll,
    )?;

    let default_secs = (hold_ms as f64 / 1000.0).max(0.5);
    let hold_ms = if behavior {
        SCROLL_HOLD_MS
    } else {
        let secs = ask_secs(default_secs)?;
        (secs * 1000.0) as u64
    };

    let url_choice = ask_select(
        "Source file/URL:",
        &[
            ("Keep current", 0u8),
            ("Pick local file (PDF, PNG, HTML)", 1),
            ("Type a URL or path", 2),
        ],
        0,
    )?;

    let new_url = match url_choice {
        1 => {
            let path = pick_local_file(
                &BrowseRoots {
                    launch_dir: std::env::current_dir().unwrap_or_else(|_| ".".into()),
                    shell_dir: std::env::current_dir().unwrap_or_else(|_| ".".into()),
                },
                false,
            )?;
            Some(file_url_absolute(&path)?)
        }
        2 => {
            let current = pane.url.as_deref().unwrap_or("");
            let raw = Text::new("URL or path:")
                .with_default(current)
                .prompt()
                .map_err(|e| Error::Export(format!("edit: {e}")))?;
            let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
            Some(repair_browser_url(raw.trim(), &cwd)?)
        }
        _ => None,
    };

    apply_placement(&mut score.layout, &pane_id, placement_pick);
    update_reveal_tail(&mut score.timeline, focus_idx, &pane_id, behavior, hold_ms);

    if let Some(url) = new_url {
        sync_browser_url(score, &pane_id, &url);
    }

    println!("  ✓ reveal updated\n");
    Ok(())
}

fn detect_placement(pane: &crate::model::Pane, layout: &crate::model::Layout) -> Placement {
    let w = layout.width;
    let h = layout.height;
    if pane.width == w && pane.height == h {
        Placement::Replace
    } else if pane.height == h && pane.width <= w.saturating_div(2).saturating_add(1) {
        Placement::SplitHorizontal
    } else if pane.width == w && pane.height <= h.saturating_div(2).saturating_add(1) {
        Placement::SplitVertical
    } else {
        Placement::Replace
    }
}

fn has_scroll_after(score: &Score, focus_idx: usize, pane_id: &str) -> bool {
    score
        .timeline
        .get(focus_idx + 1)
        .is_some_and(|s| matches!(s, Step::Scroll { pane, .. } if pane.as_deref() == Some(pane_id)))
}

fn hold_after(score: &Score, focus_idx: usize, scroll: bool) -> u64 {
    let start = focus_idx + 1 + usize::from(scroll);
    if let Some(Step::Wait { duration_ms }) = score.timeline.get(start) {
        *duration_ms
    } else if scroll {
        SCROLL_HOLD_MS
    } else {
        DEFAULT_HOLD_MS
    }
}

fn apply_placement(layout: &mut crate::model::Layout, browser_id: &str, placement: Placement) {
    let w = layout.width;
    let h = layout.height;
    let half_w = w / 2;
    let half_h = h / 2;

    let main_idx = layout.panes.iter().position(|p| p.id == "main");
    let browser_idx = layout.panes.iter().position(|p| p.id == browser_id);

    match placement {
        Placement::Replace => {
            if let Some(i) = browser_idx {
                layout.panes[i].x = 0;
                layout.panes[i].y = 0;
                layout.panes[i].width = w;
                layout.panes[i].height = h;
            }
            if let Some(i) = main_idx {
                layout.panes[i].x = 0;
                layout.panes[i].y = 0;
                layout.panes[i].width = w;
                layout.panes[i].height = h;
            }
        }
        Placement::SplitHorizontal => {
            if let Some(i) = main_idx {
                layout.panes[i].x = 0;
                layout.panes[i].y = 0;
                layout.panes[i].width = half_w;
                layout.panes[i].height = h;
            }
            if let Some(i) = browser_idx {
                layout.panes[i].x = half_w;
                layout.panes[i].y = 0;
                layout.panes[i].width = w - half_w;
                layout.panes[i].height = h;
            }
        }
        Placement::SplitVertical => {
            if let Some(i) = main_idx {
                layout.panes[i].x = 0;
                layout.panes[i].y = 0;
                layout.panes[i].width = w;
                layout.panes[i].height = half_h;
            }
            if let Some(i) = browser_idx {
                layout.panes[i].x = 0;
                layout.panes[i].y = half_h;
                layout.panes[i].width = w;
                layout.panes[i].height = h - half_h;
            }
        }
    }
}

fn update_reveal_tail(
    timeline: &mut Vec<Step>,
    focus_idx: usize,
    pane_id: &str,
    scroll: bool,
    hold_ms: u64,
) {
    let i = focus_idx + 1;
    let idx = i;
    while idx < timeline.len() {
        match &timeline[idx] {
            Step::Scroll { pane, .. } if pane.as_deref() == Some(pane_id) => {
                timeline.remove(idx);
                continue;
            }
            Step::Wait { .. } => {
                timeline.remove(idx);
                break;
            }
            _ => break,
        }
    }
    let mut at = focus_idx + 1;
    if scroll {
        timeline.insert(
            at,
            Step::Scroll {
                direction: ScrollDirection::Down,
                velocity: Velocity::Constant,
                duration_ms: hold_ms,
                pane: Some(pane_id.to_string()),
            },
        );
        at += 1;
    }
    timeline.insert(
        at,
        Step::Wait {
            duration_ms: hold_ms,
        },
    );
}

/// Keep `layout.panes`, `sources`, and any sibling browser panes in sync.
fn sync_browser_url(score: &mut Score, pane_id: &str, url: &str) {
    if let Some(p) = score.layout.panes.iter_mut().find(|p| p.id == pane_id) {
        p.url = Some(url.to_string());
    }
    // Match by pane id prefix (e.g. `pdf-r1` → source `pdf`) or exact id.
    let stem = pane_id.split("-r").next().unwrap_or(pane_id);
    for src in &mut score.sources {
        if src.id == pane_id || src.id == stem {
            src.url = Some(url.to_string());
        }
    }
}

fn ask_select<T: Copy + PartialEq>(prompt: &str, options: &[(&str, T)], current: T) -> Result<T> {
    let labels: Vec<&str> = options.iter().map(|(l, _)| *l).collect();
    let default = options
        .iter()
        .find(|(_, v)| *v == current)
        .map(|(l, _)| *l)
        .or_else(|| labels.first().copied());
    let default_idx = default
        .and_then(|d| labels.iter().position(|l| *l == d))
        .unwrap_or(0);
    let choice = Select::new(prompt, labels)
        .with_starting_cursor(default_idx)
        .prompt()
        .map_err(|e| Error::Export(format!("edit: {e}")))?;
    options
        .iter()
        .find(|(l, _)| *l == choice)
        .map(|(_, v)| *v)
        .ok_or_else(|| Error::Export("invalid choice".to_string()))
}

fn ask_secs(default: f64) -> Result<f64> {
    let secs = Text::new("Hold for how many seconds?")
        .with_default(&format!("{default:.1}"))
        .with_validator(|s: &str| match s.trim().parse::<f64>() {
            Ok(n) if n > 0.0 => Ok(inquire::validator::Validation::Valid),
            _ => Ok(inquire::validator::Validation::Invalid(
                "enter a positive number of seconds (e.g. 6)".into(),
            )),
        })
        .prompt()
        .map_err(|e| Error::Export(format!("edit: {e}")))?;
    Ok(secs.trim().parse().unwrap_or(default).max(0.5))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Layout, Pane, PaneKind, Source, SourceKind};

    fn sample_score() -> Score {
        Score {
            demo: crate::model::DemoMeta {
                name: "t".into(),
                output_dir: "./dist".into(),
                prompt: None,
                speed: None,
                targets: None,
            },
            env: None,
            typing: None,
            sources: vec![Source {
                id: "pdf".into(),
                kind: SourceKind::Browser,
                url: Some("file://./old.pdf".into()),
                theme: None,
            }],
            layout: Layout {
                width: 100,
                height: 100,
                fps: 15,
                line_height: 1.2,
                background: None,
                font_family: None,
                font_size: None,
                panes: vec![
                    Pane {
                        id: "main".into(),
                        kind: PaneKind::Terminal,
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 100,
                        font_family: None,
                        font_size: None,
                        url: None,
                        theme: None,
                        reveal_at: None,
                        hide_at: None,
                    },
                    Pane {
                        id: "pdf-r1".into(),
                        kind: PaneKind::Browser,
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 100,
                        font_family: None,
                        font_size: None,
                        url: Some("file://./old.pdf".into()),
                        theme: None,
                        reveal_at: Some(1.0),
                        hide_at: None,
                    },
                ],
            },
            timeline: vec![
                Step::Focus {
                    pane: Some("pdf-r1".into()),
                },
                Step::Wait { duration_ms: 6000 },
            ],
        }
    }

    #[test]
    fn detects_browser_focus() {
        let s = sample_score();
        assert!(is_browser_focus(&s, 0));
    }

    #[test]
    fn split_horizontal_updates_layout() {
        let mut s = sample_score();
        apply_placement(&mut s.layout, "pdf-r1", Placement::SplitHorizontal);
        let main = s.pane("main").unwrap();
        let pdf = s.pane("pdf-r1").unwrap();
        assert_eq!(main.width, 50);
        assert_eq!(pdf.x, 50);
        assert_eq!(pdf.width, 50);
    }

    #[test]
    fn scroll_tail_inserts_scroll_and_wait() {
        let mut timeline = vec![
            Step::Focus {
                pane: Some("pdf-r1".into()),
            },
            Step::Wait { duration_ms: 1000 },
        ];
        update_reveal_tail(&mut timeline, 0, "pdf-r1", true, 8000);
        assert!(matches!(
            timeline[1],
            Step::Scroll {
                duration_ms: 8000,
                ..
            }
        ));
        assert!(matches!(timeline[2], Step::Wait { duration_ms: 8000 }));
    }

    #[test]
    fn sync_url_updates_source_stem() {
        let mut s = sample_score();
        sync_browser_url(&mut s, "pdf-r1", "file://./new.pdf");
        assert_eq!(
            s.pane("pdf-r1").unwrap().url.as_deref(),
            Some("file://./new.pdf")
        );
        assert_eq!(
            s.source("pdf").unwrap().url.as_deref(),
            Some("file://./new.pdf")
        );
    }

    fn mk_layout(w: u32, h: u32) -> Layout {
        Layout {
            width: w,
            height: h,
            fps: 15,
            line_height: 1.2,
            background: None,
            font_family: None,
            font_size: None,
            panes: vec![],
        }
    }

    fn mk_pane(id: &str, w: u32, h: u32) -> Pane {
        Pane {
            id: id.into(),
            kind: PaneKind::Terminal,
            x: 0,
            y: 0,
            width: w,
            height: h,
            font_family: None,
            font_size: None,
            url: None,
            theme: None,
            reveal_at: None,
            hide_at: None,
        }
    }

    #[test]
    fn detect_placement_fullscreen() {
        let layout = mk_layout(100, 100);
        let pane = mk_pane("main", 100, 100);
        assert_eq!(detect_placement(&pane, &layout), Placement::Replace);
    }

    #[test]
    fn detect_placement_split_horizontal() {
        let layout = mk_layout(100, 100);
        let pane = mk_pane("main", 50, 100);
        assert_eq!(detect_placement(&pane, &layout), Placement::SplitHorizontal);
    }

    #[test]
    fn detect_placement_split_vertical() {
        let layout = mk_layout(100, 100);
        let pane = mk_pane("main", 100, 50);
        assert_eq!(detect_placement(&pane, &layout), Placement::SplitVertical);
    }

    #[test]
    fn detect_placement_non_standard_goes_replace() {
        let layout = mk_layout(100, 100);
        let pane = mk_pane("main", 70, 70);
        assert_eq!(detect_placement(&pane, &layout), Placement::Replace);
    }

    #[test]
    fn has_scroll_after_true_when_next_is_scroll_for_same_pane() {
        let s = Score {
            timeline: vec![
                Step::Focus {
                    pane: Some("pdf".into()),
                },
                Step::Scroll {
                    direction: ScrollDirection::Down,
                    velocity: Velocity::Constant,
                    duration_ms: 1000,
                    pane: Some("pdf".into()),
                },
            ],
            ..sample_score()
        };
        assert!(has_scroll_after(&s, 0, "pdf"));
    }

    #[test]
    fn has_scroll_after_false_when_no_scroll() {
        let s = Score {
            timeline: vec![
                Step::Focus {
                    pane: Some("pdf".into()),
                },
                Step::Wait { duration_ms: 1000 },
            ],
            ..sample_score()
        };
        assert!(!has_scroll_after(&s, 0, "pdf"));
    }

    #[test]
    fn has_scroll_after_false_when_wrong_pane() {
        let s = Score {
            timeline: vec![
                Step::Focus {
                    pane: Some("pdf".into()),
                },
                Step::Scroll {
                    direction: ScrollDirection::Down,
                    velocity: Velocity::Constant,
                    duration_ms: 1000,
                    pane: Some("other".into()),
                },
            ],
            ..sample_score()
        };
        assert!(!has_scroll_after(&s, 0, "pdf"));
    }

    #[test]
    fn hold_after_returns_wait_duration() {
        let s = Score {
            timeline: vec![
                Step::Focus {
                    pane: Some("pdf".into()),
                },
                Step::Wait { duration_ms: 4200 },
            ],
            ..sample_score()
        };
        assert_eq!(hold_after(&s, 0, false), 4200);
    }

    #[test]
    fn hold_after_skips_scroll_when_present() {
        let s = Score {
            timeline: vec![
                Step::Focus {
                    pane: Some("pdf".into()),
                },
                Step::Scroll {
                    direction: ScrollDirection::Down,
                    velocity: Velocity::Constant,
                    duration_ms: 8000,
                    pane: Some("pdf".into()),
                },
                Step::Wait { duration_ms: 3000 },
            ],
            ..sample_score()
        };
        assert_eq!(hold_after(&s, 0, true), 3000);
    }

    #[test]
    fn hold_after_defaults_for_scroll_without_wait() {
        let s = Score {
            timeline: vec![
                Step::Focus {
                    pane: Some("pdf".into()),
                },
                Step::Scroll {
                    direction: ScrollDirection::Down,
                    velocity: Velocity::Constant,
                    duration_ms: 8000,
                    pane: Some("pdf".into()),
                },
            ],
            ..sample_score()
        };
        assert_eq!(hold_after(&s, 0, true), SCROLL_HOLD_MS);
    }

    #[test]
    fn hold_after_defaults_for_no_scroll_no_wait() {
        let s = sample_score();
        assert_eq!(hold_after(&s, 0, false), DEFAULT_HOLD_MS);
    }

    #[test]
    fn replace_placement_updates_both_panes() {
        let mut s = sample_score();
        apply_placement(&mut s.layout, "pdf-r1", Placement::Replace);
        let main = s.pane("main").unwrap();
        let pdf = s.pane("pdf-r1").unwrap();
        assert_eq!(main.width, 100);
        assert_eq!(main.height, 100);
        assert_eq!(pdf.width, 100);
        assert_eq!(pdf.height, 100);
    }

    #[test]
    fn split_vertical_updates_layout() {
        let mut s = sample_score();
        apply_placement(&mut s.layout, "pdf-r1", Placement::SplitVertical);
        let main = s.pane("main").unwrap();
        let pdf = s.pane("pdf-r1").unwrap();
        assert_eq!(main.height, 50);
        assert_eq!(pdf.y, 50);
        assert_eq!(pdf.height, 50);
    }

    #[test]
    fn is_browser_focus_false_for_terminal_pane() {
        let mut s = sample_score();
        s.timeline[0] = Step::Focus {
            pane: Some("main".into()),
        };
        assert!(!is_browser_focus(&s, 0));
    }

    #[test]
    fn is_browser_focus_false_for_none_pane() {
        let mut s = sample_score();
        s.timeline[0] = Step::Focus { pane: None };
        assert!(!is_browser_focus(&s, 0));
    }

    #[test]
    fn update_reveal_tail_no_scroll_only_wait() {
        let mut timeline = vec![
            Step::Focus {
                pane: Some("pdf-r1".into()),
            },
            Step::Wait { duration_ms: 1000 },
        ];
        update_reveal_tail(&mut timeline, 0, "pdf-r1", false, 5000);
        assert_eq!(timeline.len(), 2);
        assert!(matches!(timeline[1], Step::Wait { duration_ms: 5000 }));
    }

    #[test]
    fn update_reveal_tail_removes_existing_scroll_for_same_pane() {
        let mut timeline = vec![
            Step::Focus {
                pane: Some("pdf-r1".into()),
            },
            Step::Scroll {
                direction: ScrollDirection::Down,
                velocity: Velocity::Constant,
                duration_ms: 8000,
                pane: Some("pdf-r1".into()),
            },
            Step::Wait { duration_ms: 8000 },
            Step::Type {
                text: "keep".into(),
                human_salt: false,
            },
        ];
        update_reveal_tail(&mut timeline, 0, "pdf-r1", true, 5000);
        assert!(matches!(timeline[1], Step::Scroll { .. }));
        assert!(matches!(timeline[2], Step::Wait { duration_ms: 5000 }));
        assert!(matches!(timeline[3], Step::Type { .. }));
    }

    #[test]
    fn update_reveal_tail_removes_multiple_scrolls_for_same_pane() {
        let mut timeline = vec![
            Step::Focus {
                pane: Some("pdf-r1".into()),
            },
            Step::Scroll {
                direction: ScrollDirection::Down,
                velocity: Velocity::Constant,
                duration_ms: 2000,
                pane: Some("pdf-r1".into()),
            },
            Step::Scroll {
                direction: ScrollDirection::Down,
                velocity: Velocity::Constant,
                duration_ms: 3000,
                pane: Some("pdf-r1".into()),
            },
            Step::Wait { duration_ms: 8000 },
        ];
        update_reveal_tail(&mut timeline, 0, "pdf-r1", true, 5000);
        // Should have: Focus, Scroll (new), Wait (new)
        assert_eq!(timeline.len(), 3);
        assert!(matches!(
            timeline[1],
            Step::Scroll {
                duration_ms: 5000,
                ..
            }
        ));
        assert!(matches!(timeline[2], Step::Wait { duration_ms: 5000 }));
    }

    #[test]
    fn update_reveal_tail_keeps_scroll_for_different_pane() {
        let mut timeline = vec![
            Step::Focus {
                pane: Some("pdf-r1".into()),
            },
            Step::Scroll {
                direction: ScrollDirection::Down,
                velocity: Velocity::Constant,
                duration_ms: 4000,
                pane: Some("other".into()),
            },
            Step::Wait { duration_ms: 2000 },
        ];
        update_reveal_tail(&mut timeline, 0, "pdf-r1", false, 6000);
        // The scroll for "other" stays, then a new Wait is inserted after Focus
        let has_scroll_other = timeline.iter().any(|s| {
            matches!(
                s, Step::Scroll { pane: Some(ref p), .. } if p == "other"
            )
        });
        assert!(has_scroll_other, "scroll for different pane should remain");
        // A new Wait should be inserted
        let waits: Vec<_> = timeline
            .iter()
            .filter(|s| matches!(s, Step::Wait { .. }))
            .collect();
        assert_eq!(waits.len(), 2, "should have two waits (new + old)");
    }

    #[test]
    fn update_reveal_tail_empty_timeline_after_focus() {
        let mut timeline = vec![Step::Focus {
            pane: Some("pdf-r1".into()),
        }];
        update_reveal_tail(&mut timeline, 0, "pdf-r1", false, 3000);
        assert_eq!(timeline.len(), 2);
        assert!(matches!(timeline[1], Step::Wait { duration_ms: 3000 }));
    }

    #[test]
    fn sync_browser_url_updates_matching_prefix() {
        let mut s = sample_score();
        sync_browser_url(&mut s, "pdf-r2", "file://./v2.pdf");
        // Should update the source with id "pdf" (stem of "pdf-r2")
        assert_eq!(
            s.source("pdf").unwrap().url.as_deref(),
            Some("file://./v2.pdf")
        );
    }

    #[test]
    fn sync_browser_url_no_source_match() {
        let mut s = sample_score();
        sync_browser_url(&mut s, "unknown", "file://./x.pdf");
        // Source "pdf" should not be updated
        assert_eq!(
            s.source("pdf").unwrap().url.as_deref(),
            Some("file://./old.pdf")
        );
    }

    #[test]
    fn detect_placement_exact_half_width() {
        let layout = mk_layout(100, 100);
        // width = 50 = exactly half → SplitHorizontal
        let pane = mk_pane("main", 50, 100);
        assert_eq!(detect_placement(&pane, &layout), Placement::SplitHorizontal);
    }

    #[test]
    fn detect_placement_exact_half_height() {
        let layout = mk_layout(100, 100);
        // height = 50 = exactly half → SplitVertical
        let pane = mk_pane("main", 100, 50);
        assert_eq!(detect_placement(&pane, &layout), Placement::SplitVertical);
    }

    #[test]
    fn detect_placement_wider_than_half() {
        let layout = mk_layout(100, 100);
        // width = 60, height = 100 → not half-width (60 > 50), so Replace
        let pane = mk_pane("main", 60, 100);
        assert_eq!(detect_placement(&pane, &layout), Placement::Replace);
    }

    #[test]
    fn detect_placement_taller_than_half() {
        let layout = mk_layout(100, 100);
        // width = 100, height = 60 → not half-height (60 > 50), so Replace
        let pane = mk_pane("main", 100, 60);
        assert_eq!(detect_placement(&pane, &layout), Placement::Replace);
    }
}
