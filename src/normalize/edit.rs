//! Keystroke reconstruction (SPEC §3.1): replay the raw input stream through a
//! minimal line editor, applying destructive edits (backspace, Ctrl-U, Ctrl-C)
//! so the result is the *clean* text the user meant to run — never the typos.

/// A reconstructed command line.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    /// The clean text, after pruning destructive edits.
    pub text: String,
    /// Time (ms from start) the first character was typed.
    pub start_ms: u64,
    /// Time (ms from start) `Enter` was pressed (`== start_ms` if never).
    pub enter_ms: u64,
    /// Whether the line was submitted with `Enter`.
    pub had_enter: bool,
}

/// Where we are inside a terminal escape sequence. Arrow keys and other special
/// keys arrive as multi-byte sequences (e.g. Up = `ESC [ A`); the whole sequence
/// must be swallowed, or the non-control tail (`[A`, `[B`, …) leaks into the
/// reconstructed text as if the user had typed it.
enum Esc {
    /// Not in a sequence.
    None,
    /// Just saw `ESC`; the next byte selects the sequence kind.
    Saw,
    /// Inside a CSI sequence (`ESC [ … final`); consume up to the final byte.
    Csi,
    /// Inside an SS3 sequence (`ESC O x`); exactly one byte follows.
    Ss3,
}

/// Replay timestamped input chunks into a list of clean commands. Blank lines
/// (a bare `Enter`, or a line killed before submitting) are dropped.
pub fn reconstruct(inputs: &[(u64, &str)]) -> Vec<Command> {
    let mut commands = Vec::new();
    let mut buf: Vec<char> = Vec::new();
    let mut start_ms: Option<u64> = None;
    let mut esc = Esc::None;

    let flush = |commands: &mut Vec<Command>,
                 buf: &mut Vec<char>,
                 start: u64,
                 enter: u64,
                 had_enter: bool| {
        let text: String = buf.iter().collect();
        if !text.trim().is_empty() {
            commands.push(Command {
                text,
                start_ms: start,
                enter_ms: enter,
                had_enter,
            });
        }
        buf.clear();
    };

    for &(t, bytes) in inputs {
        for ch in bytes.chars() {
            // Swallow special-key escape sequences whole (arrows, Home/End, F-keys),
            // so their tail (`[A`, `[B`, `OP`, …) never lands in the command text.
            match esc {
                Esc::Csi => {
                    // CSI ends at a final byte in 0x40..=0x7E.
                    if ('\u{40}'..='\u{7e}').contains(&ch) {
                        esc = Esc::None;
                    }
                    continue;
                }
                Esc::Ss3 => {
                    esc = Esc::None;
                    continue;
                }
                Esc::Saw => {
                    esc = match ch {
                        '[' => Esc::Csi,
                        'O' => Esc::Ss3,
                        // Any other byte: a 2-byte escape (e.g. Alt-key); done.
                        _ => Esc::None,
                    };
                    continue;
                }
                Esc::None => {}
            }
            match ch {
                // ESC: start of a special-key sequence — swallow what follows.
                '\u{1b}' => esc = Esc::Saw,
                '\r' | '\n' => {
                    flush(&mut commands, &mut buf, start_ms.unwrap_or(t), t, true);
                    start_ms = None;
                }
                // Backspace / DEL.
                '\u{7f}' | '\u{8}' => {
                    buf.pop();
                }
                // Ctrl-U: kill the whole line.
                '\u{15}' => buf.clear(),
                // Ctrl-C: cancel the line entirely.
                '\u{3}' => {
                    buf.clear();
                    start_ms = None;
                }
                // Ignore other stray control bytes (Tab, Bell, …).
                c if c.is_control() => {}
                c => {
                    if start_ms.is_none() {
                        start_ms = Some(t);
                    }
                    buf.push(c);
                }
            }
        }
    }

    // A trailing, unsubmitted line (typed but no Enter).
    if let Some(start) = start_ms {
        flush(&mut commands, &mut buf, start, start, false);
    }

    commands
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prunes_backspaces() {
        // g t i [bs] [bs] i t  →  "git"
        let cmds = reconstruct(&[(0, "gti\u{7f}\u{7f}it\r")]);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].text, "git");
        assert!(cmds[0].had_enter);
    }

    #[test]
    fn ctrl_u_kills_the_line() {
        let cmds = reconstruct(&[(0, "wrong command\u{15}ls -la\r")]);
        assert_eq!(cmds[0].text, "ls -la");
    }

    #[test]
    fn splits_commands_on_enter() {
        let cmds = reconstruct(&[(0, "ls\r"), (500, "pwd\r")]);
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].text, "ls");
        assert_eq!(cmds[1].text, "pwd");
        assert_eq!(cmds[1].start_ms, 500);
    }

    #[test]
    fn drops_blank_lines() {
        let cmds = reconstruct(&[(0, "\r\r"), (10, "  \r"), (20, "echo hi\r")]);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].text, "echo hi");
    }

    #[test]
    fn keeps_trailing_unsubmitted_line() {
        let cmds = reconstruct(&[(0, "ls\r"), (300, "vim")]);
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[1].text, "vim");
        assert!(!cmds[1].had_enter);
    }

    #[test]
    fn swallows_arrow_key_escape_sequences() {
        // Up, Down, Right, Left around real text must not leak `[A[B…` as typed.
        let cmds = reconstruct(&[(0, "ab\u{1b}[A\u{1b}[Bcd\u{1b}[C\u{1b}[De\r")]);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].text, "abcde");
    }

    #[test]
    fn swallows_ss3_and_csi_tilde_sequences() {
        // SS3 (F1 = ESC O P) and a CSI tilde key (F5 = ESC [ 1 5 ~) leave nothing.
        let cmds = reconstruct(&[(0, "x\u{1b}OP\u{1b}[15~y\r")]);
        assert_eq!(cmds[0].text, "xy");
    }

    #[test]
    fn tracks_typing_start_time() {
        let cmds = reconstruct(&[(100, "a"), (140, "b"), (900, "c\r")]);
        assert_eq!(cmds[0].start_ms, 100);
        assert_eq!(cmds[0].enter_ms, 900);
    }
}
