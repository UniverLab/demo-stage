//! Keystroke reconstruction (SPEC §3.1): replay the raw input stream through a
//! minimal line editor, producing an ordered stream of [`Action`]s — clean typed
//! text plus the keys actually pressed (Enter, arrows, Tab, …).
//!
//! Destructive edits (backspace, Ctrl-U) are applied so typed text is the *clean*
//! text the user meant — never the typos. But navigation keys are **kept** as
//! [`Action::Key`], so re-executing the score (`demo record`) drives interactive
//! programs — wizards, selectors — exactly as the human did, instead of
//! desyncing and cancelling.

/// One reconstructed input action, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// A run of clean typed text (destructive edits already applied).
    Type {
        text: String,
        /// Time (ms from start) the first character of the run was typed.
        t_ms: u64,
        /// Time (ms from start) the last character of the run was typed.
        end_ms: u64,
    },
    /// A single named key press (`enter`, `up`, `down`, `tab`, `ctrl+c`, …).
    Key {
        key: String,
        /// Time (ms from start) the key was pressed.
        t_ms: u64,
    },
}

/// Where we are inside a terminal escape sequence. Arrow keys and other special
/// keys arrive as multi-byte sequences (e.g. Up = `ESC [ A`); we parse the whole
/// sequence and turn it into a named [`Action::Key`].
enum Esc {
    /// Not in a sequence.
    None,
    /// Just saw `ESC`; the next byte selects the sequence kind.
    Saw,
    /// Inside a CSI sequence (`ESC [ … final`); collecting parameter bytes.
    Csi(String),
    /// Inside an SS3 sequence (`ESC O x`) — application-mode cursor / function keys.
    Ss3,
    /// Inside an OSC (`ESC ] … BEL` or `ESC ] … ESC \`) — terminal colour queries,
    /// title changes, etc. TUI apps (e.g. opencode) trigger these and the host
    /// terminal may echo the responses into the captured input stream.
    Osc,
    /// Inside a DCS (`ESC P … ESC \`), APC, PM, or SOS string.
    Str,
    /// Saw `ESC` while inside [`Esc::Osc`] / [`Esc::Str`] — expect `\` (ST).
    St,
    /// Bracketed-paste payload between `ESC [ 200 ~` and `ESC [ 201 ~`.
    Paste,
    /// Saw `ESC` inside a bracketed paste — expect `[` to start the end marker.
    PasteSaw,
    /// Collecting CSI params for a bracketed-paste end marker (`201 ~`).
    PasteCsi(String),
}

/// Map a CSI sequence (its parameters and final byte) to a key name, if known.
fn csi_key(params: &str, final_byte: char) -> Option<&'static str> {
    match final_byte {
        'A' => Some("up"),
        'B' => Some("down"),
        'C' => Some("right"),
        'D' => Some("left"),
        'H' => Some("home"),
        'F' => Some("end"),
        '~' => match params {
            "1" | "7" => Some("home"),
            "4" | "8" => Some("end"),
            "3" => Some("delete"),
            "5" => Some("pageup"),
            "6" => Some("pagedown"),
            _ => None,
        },
        _ => None,
    }
}

/// Map an SS3 sequence's final byte to a key name (application-mode arrows), if known.
fn ss3_key(final_byte: char) -> Option<&'static str> {
    match final_byte {
        'A' => Some("up"),
        'B' => Some("down"),
        'C' => Some("right"),
        'D' => Some("left"),
        'H' => Some("home"),
        'F' => Some("end"),
        _ => None,
    }
}

/// Drop OSC bodies that leaked without their `ESC ]` prefix — the old parser
/// consumed `ESC` and discarded `]`, leaving fragments like `10;rgb:…` in the
/// typed text.
fn strip_orphan_osc_bodies(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if let Some(skip) = orphan_osc_cluster_len(&chars[i..]) {
            i += skip;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// One OSC colour response: `10;rgb:abab/baba/baba` or `4;0;rgb:0000/0000/0000`.
fn orphan_osc_unit_len(chars: &[char]) -> Option<usize> {
    let mut i = 0;
    if chars.first().is_none_or(|c| !c.is_ascii_digit()) {
        return None;
    }
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    loop {
        if chars.get(i) != Some(&';') {
            break;
        }
        i += 1;
        let param_start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i == param_start {
            break;
        }
    }
    if chars.get(i..i + 4) != Some(&['r', 'g', 'b', ':']) {
        return None;
    }
    i += 4;
    for comp in 0..3 {
        let start = i;
        while i < chars.len() && chars[i].is_ascii_hexdigit() && i - start < 4 {
            i += 1;
        }
        if i == start {
            return None;
        }
        if comp < 2 {
            if chars.get(i) != Some(&'/') {
                return None;
            }
            i += 1;
        }
    }
    Some(i)
}

/// A run of one or more concatenated OSC colour responses.
fn orphan_osc_cluster_len(chars: &[char]) -> Option<usize> {
    let mut total = 0;
    loop {
        match orphan_osc_unit_len(&chars[total..]) {
            Some(n) => total += n,
            None => {
                if total > 0 && chars.get(total) == Some(&';') {
                    if let Some(n) = orphan_osc_unit_len(&chars[total + 1..]) {
                        total += 1 + n;
                        continue;
                    }
                }
                break;
            }
        }
    }
    if total > 0 {
        Some(total)
    } else {
        None
    }
}

/// Replay timestamped input chunks into an ordered list of clean actions.
pub fn reconstruct(inputs: &[(u64, &str)]) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut buf: Vec<char> = Vec::new();
    let mut start_ms = 0u64;
    let mut end_ms = 0u64;
    let mut esc = Esc::None;

    // Emit the pending typed run (if any) as a Type action.
    let flush = |actions: &mut Vec<Action>, buf: &mut Vec<char>, start: u64, end: u64| {
        if !buf.is_empty() {
            actions.push(Action::Type {
                text: buf.iter().collect(),
                t_ms: start,
                end_ms: end,
            });
            buf.clear();
        }
    };

    for &(t, bytes) in inputs {
        let bytes = strip_orphan_osc_bodies(bytes);
        for ch in bytes.chars() {
            match esc {
                Esc::Csi(ref mut params) => {
                    if ('\u{40}'..='\u{7e}').contains(&ch) {
                        if ch == '~' && params == "200" {
                            esc = Esc::Paste;
                        } else if let Some(key) = csi_key(params, ch) {
                            flush(&mut actions, &mut buf, start_ms, end_ms);
                            actions.push(Action::Key {
                                key: key.to_string(),
                                t_ms: t,
                            });
                            esc = Esc::None;
                        } else {
                            esc = Esc::None;
                        }
                    } else {
                        params.push(ch);
                    }
                    continue;
                }
                Esc::Ss3 => {
                    if let Some(key) = ss3_key(ch) {
                        flush(&mut actions, &mut buf, start_ms, end_ms);
                        actions.push(Action::Key {
                            key: key.to_string(),
                            t_ms: t,
                        });
                    }
                    esc = Esc::None;
                    continue;
                }
                Esc::Osc | Esc::Str => {
                    match ch {
                        '\u{07}' => esc = Esc::None,
                        '\u{1b}' => esc = Esc::St,
                        _ => {}
                    }
                    continue;
                }
                Esc::St => {
                    esc = Esc::None;
                    continue;
                }
                Esc::Paste => {
                    if ch == '\u{1b}' {
                        esc = Esc::PasteSaw;
                    } else if ch == '\r' || ch == '\n' {
                        flush(&mut actions, &mut buf, start_ms, end_ms);
                        actions.push(Action::Key {
                            key: "enter".to_string(),
                            t_ms: t,
                        });
                    } else if ch == '\u{7f}' || ch == '\u{8}' {
                        buf.pop();
                    } else if ch == '\u{15}' {
                        buf.clear();
                    } else if !ch.is_control() {
                        if buf.is_empty() {
                            start_ms = t;
                        }
                        end_ms = t;
                        buf.push(ch);
                    }
                    continue;
                }
                Esc::PasteSaw => {
                    esc = if ch == '[' {
                        Esc::PasteCsi(String::new())
                    } else {
                        Esc::Paste
                    };
                    continue;
                }
                Esc::PasteCsi(ref mut params) => {
                    if ('\u{40}'..='\u{7e}').contains(&ch) {
                        esc = if ch == '~' && params == "201" {
                            Esc::None
                        } else {
                            Esc::Paste
                        };
                    } else {
                        params.push(ch);
                    }
                    continue;
                }
                Esc::Saw => {
                    esc = match ch {
                        '[' => Esc::Csi(String::new()),
                        'O' => Esc::Ss3,
                        ']' => Esc::Osc,
                        'P' => Esc::Str,
                        '_' | '^' | 'X' => Esc::Str,
                        // ESC + char (Alt-key) — uncommon in demos; drop it.
                        _ => Esc::None,
                    };
                    continue;
                }
                Esc::None => {}
            }
            match ch {
                // ESC: start of a special-key sequence.
                '\u{1b}' => esc = Esc::Saw,
                // Enter: submit. ALWAYS kept — a bare Enter accepts a selector
                // default, which interactive programs depend on.
                '\r' | '\n' => {
                    flush(&mut actions, &mut buf, start_ms, end_ms);
                    actions.push(Action::Key {
                        key: "enter".to_string(),
                        t_ms: t,
                    });
                }
                // Backspace / DEL: prune a typo from the current run.
                '\u{7f}' | '\u{8}' => {
                    buf.pop();
                }
                // Ctrl-U: kill the whole line.
                '\u{15}' => buf.clear(),
                // Ctrl-C: cancel — kept as a key so replay matches.
                '\u{3}' => {
                    buf.clear();
                    actions.push(Action::Key {
                        key: "ctrl+c".to_string(),
                        t_ms: t,
                    });
                }
                // Tab: completion / field navigation — kept as a key.
                '\t' => {
                    flush(&mut actions, &mut buf, start_ms, end_ms);
                    actions.push(Action::Key {
                        key: "tab".to_string(),
                        t_ms: t,
                    });
                }
                // Ignore other stray control bytes (Bell, …).
                c if c.is_control() => {}
                c => {
                    if buf.is_empty() {
                        start_ms = t;
                    }
                    end_ms = t;
                    buf.push(c);
                }
            }
        }
    }
    flush(&mut actions, &mut buf, start_ms, end_ms);
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(actions: &[Action]) -> Vec<&str> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Type { text, .. } => Some(text.as_str()),
                Action::Key { .. } => None,
            })
            .collect()
    }

    fn keys(actions: &[Action]) -> Vec<&str> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Key { key, .. } => Some(key.as_str()),
                Action::Type { .. } => None,
            })
            .collect()
    }

    #[test]
    fn prunes_backspaces() {
        // g t i [bs] [bs] i t  →  "git"
        let a = reconstruct(&[(0, "gti\u{7f}\u{7f}it\r")]);
        assert_eq!(typed(&a), vec!["git"]);
        assert_eq!(keys(&a), vec!["enter"]);
    }

    #[test]
    fn ctrl_u_kills_the_line() {
        let a = reconstruct(&[(0, "wrong command\u{15}ls -la\r")]);
        assert_eq!(typed(&a), vec!["ls -la"]);
    }

    #[test]
    fn splits_runs_on_enter() {
        let a = reconstruct(&[(0, "ls\r"), (500, "pwd\r")]);
        assert_eq!(typed(&a), vec!["ls", "pwd"]);
        assert_eq!(keys(&a), vec!["enter", "enter"]);
    }

    #[test]
    fn keeps_bare_enters() {
        // A lone Enter (accepting a selector default) must be preserved.
        let a = reconstruct(&[(0, "\r"), (10, "echo hi\r")]);
        assert_eq!(typed(&a), vec!["echo hi"]);
        assert_eq!(keys(&a), vec!["enter", "enter"]);
    }

    #[test]
    fn keeps_arrow_keys_as_keypresses() {
        // Navigation in a selector: typed text stays clean, arrows become keys.
        let a = reconstruct(&[(0, "ab\u{1b}[A\u{1b}[Bcd\u{1b}[C\u{1b}[De\r")]);
        assert_eq!(typed(&a), vec!["ab", "cd", "e"]);
        assert_eq!(keys(&a), vec!["up", "down", "right", "left", "enter"]);
    }

    #[test]
    fn maps_application_mode_arrows() {
        // SS3 arrows (ESC O A/B) used in application cursor mode.
        let a = reconstruct(&[(0, "\u{1b}OA\u{1b}OB\r")]);
        assert_eq!(keys(&a), vec!["up", "down", "enter"]);
    }

    #[test]
    fn tracks_typing_start_time() {
        let a = reconstruct(&[(100, "a"), (140, "b"), (900, "c\r")]);
        match &a[0] {
            Action::Type { text, t_ms, .. } => {
                assert_eq!(text, "abc");
                assert_eq!(*t_ms, 100);
            }
            _ => panic!("expected a Type action first"),
        }
    }

    #[test]
    fn drops_osc_colour_query_responses() {
        // TUI apps query palette colours; the host terminal may echo the OSC
        // responses into the captured input — they must not become demo typing.
        let osc = "\u{1b}]10;rgb:baba/b7b7/b6b6\u{1b}\\";
        let a = reconstruct(&[(0, &format!("{osc}create a thesis\r"))]);
        assert_eq!(typed(&a), vec!["create a thesis"]);
        assert_eq!(keys(&a), vec!["enter"]);
    }

    #[test]
    fn drops_many_concatenated_osc_responses() {
        let noise = "10;rgb:baba/b7b7/b6b611;rgb:1414/1414/14144;0;rgb:0000/0000/0000";
        let osc = format!("\u{1b}]{noise}\u{1b}\\");
        let a = reconstruct(&[(0, &format!("{osc}/exit\r"))]);
        assert_eq!(typed(&a), vec!["/exit"]);
    }

    #[test]
    fn strip_orphan_osc_bodies_unit() {
        let noise = "10;rgb:baba/b7b7/b6b611;rgb:1414/1414/14144;0;rgb:0000/0000/0000";
        assert_eq!(strip_orphan_osc_bodies(noise), "");
        assert_eq!(strip_orphan_osc_bodies(&format!("{noise}/exit")), "/exit");
    }

    #[test]
    fn drops_orphan_osc_bodies_without_esc_prefix() {
        // Regression: the old parser ate ESC/] and left the OSC payload behind.
        let noise = "10;rgb:baba/b7b7/b6b611;rgb:1414/1414/14144;0;rgb:0000/0000/0000";
        let a = reconstruct(&[(0, &format!("{noise}/exit\r"))]);
        assert_eq!(typed(&a), vec!["/exit"]);
    }

    #[test]
    fn keeps_bracketed_paste_payload() {
        let paste = "\u{1b}[200~hello from paste\u{1b}[201~";
        let a = reconstruct(&[(0, &format!("{paste}\r"))]);
        assert_eq!(typed(&a), vec!["hello from paste"]);
    }
}
