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

/// Modifier bitmask values in CSI sequences (xterm-style).
const MOD_SHIFT: u8 = 2;
const MOD_ALT: u8 = 3;
const MOD_CTRL: u8 = 5;
const MOD_CTRL_SHIFT: u8 = 6;

/// Decode a CSI modifier number into a prefix string like "shift-", "alt-", "ctrl+", etc.
fn modifier_prefix(modifier: u8) -> &'static str {
    match modifier {
        MOD_SHIFT => "shift-",
        MOD_ALT => "alt-",
        4 => "alt+shift-", // alt(2) + shift but bitmask 4 in some terminals
        MOD_CTRL => "ctrl+",
        MOD_CTRL_SHIFT => "ctrl+shift-",
        7 => "ctrl+alt-",
        8 => "ctrl+alt+shift-",
        _ => "",
    }
}

/// Parse CSI params for a modified key: returns `(keycode, modifier)`. The
/// modifier defaults to 0 (unmodified) when absent.
fn parse_csi_params(params: &str) -> (u8, u8) {
    if let Some((code_str, mod_str)) = params.split_once(';') {
        let code = code_str.parse::<u8>().unwrap_or(0);
        let modifier = mod_str.parse::<u8>().unwrap_or(0);
        (code, modifier)
    } else {
        let code = params.parse::<u8>().unwrap_or(0);
        (code, 0)
    }
}

/// Map a CSI sequence (its parameters and final byte) to a key name, if known.
/// Handles both unmodified keys (`\x1b[A`) and modifier combinations
/// (`\x1b[1;2A` = Shift+Up, `\x1b[1;5A` = Ctrl+Up, etc.).
fn csi_key(params: &str, final_byte: char) -> Option<String> {
    let (code, modifier) = parse_csi_params(params);
    let prefix = modifier_prefix(modifier);

    let base = match final_byte {
        'A' => Some("up"),
        'B' => Some("down"),
        'C' => Some("right"),
        'D' => Some("left"),
        'H' => Some("home"),
        'F' => Some("end"),
        'P' if modifier == 0 => Some("f1"),
        'Q' if modifier == 0 => Some("f2"),
        'R' if modifier == 0 => Some("f3"),
        'S' if modifier == 0 => Some("f4"),
        '~' => match code {
            1 | 7 => Some("home"),
            4 | 8 => Some("end"),
            3 => Some("delete"),
            5 => Some("pageup"),
            6 => Some("pagedown"),
            // VT220 function keys F1-F12
            11 => Some("f1"),
            12 => Some("f2"),
            13 => Some("f3"),
            14 => Some("f4"),
            15 => Some("f5"),
            17 => Some("f6"),
            18 => Some("f7"),
            19 => Some("f8"),
            20 => Some("f9"),
            21 => Some("f10"),
            23 => Some("f11"),
            24 => Some("f12"),
            // Insert / Delete with modifiers
            2 => Some("insert"),
            _ => None,
        },
        _ => None,
    };
    base.map(|b| format!("{prefix}{b}"))
}

/// Map an SS3 sequence's final byte to a key name (application-mode arrows and function keys), if known.
fn ss3_key(final_byte: char) -> Option<&'static str> {
    match final_byte {
        'A' => Some("up"),
        'B' => Some("down"),
        'C' => Some("right"),
        'D' => Some("left"),
        'H' => Some("home"),
        'F' => Some("end"),
        // Application-mode function keys F1-F4 (ESC O P/Q/R/S)
        'P' => Some("f1"),
        'Q' => Some("f2"),
        'R' => Some("f3"),
        'S' => Some("f4"),
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
                            actions.push(Action::Key { key, t_ms: t });
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
                    match ch {
                        '[' => {
                            esc = Esc::Csi(String::new());
                            continue;
                        }
                        'O' => {
                            esc = Esc::Ss3;
                            continue;
                        }
                        ']' => {
                            esc = Esc::Osc;
                            continue;
                        }
                        'P' => {
                            esc = Esc::Str;
                            continue;
                        }
                        '_' | '^' | 'X' => {
                            esc = Esc::Str;
                            continue;
                        }
                        _ => {
                            // ESC + unrecognized char: emit "esc" as a keypress
                            // and process the following character normally.
                            flush(&mut actions, &mut buf, start_ms, end_ms);
                            actions.push(Action::Key {
                                key: "esc".to_string(),
                                t_ms: t,
                            });
                            esc = Esc::None;
                            // Fall through to the main match ch below.
                        }
                    }
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
                // Ctrl-S: XOFF / save — kept as a key so replay matches.
                '\u{13}' => {
                    flush(&mut actions, &mut buf, start_ms, end_ms);
                    actions.push(Action::Key {
                        key: "ctrl+s".to_string(),
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
    // A bare ESC at the end of input (no following character) — emit it as a
    // keypress so it doesn't get silently dropped.
    if let Esc::Saw = esc {
        actions.push(Action::Key {
            key: "esc".to_string(),
            t_ms: end_ms,
        });
    }
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

    #[test]
    fn maps_function_keys_csi() {
        // F2 = ESC [ 1 2 ~, F5 = ESC [ 1 5 ~, F12 = ESC [ 2 4 ~
        let a = reconstruct(&[(0, "\u{1b}[12~\u{1b}[15~\u{1b}[24~\r")]);
        assert_eq!(keys(&a), vec!["f2", "f5", "f12", "enter"]);
    }

    #[test]
    fn maps_function_keys_ss3() {
        // F1-F4 in application mode: ESC O P/Q/R/S
        let a = reconstruct(&[(0, "\u{1b}OP\u{1b}OQ\u{1b}OR\u{1b}OS\r")]);
        assert_eq!(keys(&a), vec!["f1", "f2", "f3", "f4", "enter"]);
    }

    #[test]
    fn captures_bare_esc() {
        // A standalone ESC keypress (no following character).
        let a = reconstruct(&[(0, "\u{1b}")]);
        assert_eq!(keys(&a), vec!["esc"]);
    }

    #[test]
    fn captures_esc_followed_by_text() {
        // ESC followed by a normal character: emit "esc" then type the char.
        let a = reconstruct(&[(0, "\u{1b}x")]);
        assert_eq!(keys(&a), vec!["esc"]);
        assert_eq!(typed(&a), vec!["x"]);
    }

    #[test]
    fn captures_ctrl_s() {
        // Ctrl-S (0x13) should be captured as a keypress, not dropped.
        let a = reconstruct(&[(0, "hello\u{13}world\r")]);
        assert_eq!(typed(&a), vec!["hello", "world"]);
        assert_eq!(keys(&a), vec!["ctrl+s", "enter"]);
    }

    #[test]
    fn maps_shift_arrows() {
        // Shift+Up = ESC [ 1 ; 2 A, Shift+Left = ESC [ 1 ; 2 D
        let a = reconstruct(&[(0, "\u{1b}[1;2A\u{1b}[1;2D\r")]);
        assert_eq!(keys(&a), vec!["shift-up", "shift-left", "enter"]);
    }

    #[test]
    fn maps_ctrl_arrows() {
        // Ctrl+Right = ESC [ 1 ; 5 C
        let a = reconstruct(&[(0, "\u{1b}[1;5C\r")]);
        assert_eq!(keys(&a), vec!["ctrl+right", "enter"]);
    }

    #[test]
    fn maps_alt_arrows() {
        // Alt+Down = ESC [ 1 ; 3 B
        let a = reconstruct(&[(0, "\u{1b}[1;3B\r")]);
        assert_eq!(keys(&a), vec!["alt-down", "enter"]);
    }

    #[test]
    fn maps_modified_function_keys() {
        // Shift+F5 = ESC [ 1 5 ; 2 ~
        let a = reconstruct(&[(0, "\u{1b}[15;2~\r")]);
        assert_eq!(keys(&a), vec!["shift-f5", "enter"]);
    }
}
