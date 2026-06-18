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

/// Replay timestamped input chunks into a list of clean commands. Blank lines
/// (a bare `Enter`, or a line killed before submitting) are dropped.
pub fn reconstruct(inputs: &[(u64, &str)]) -> Vec<Command> {
    let mut commands = Vec::new();
    let mut buf: Vec<char> = Vec::new();
    let mut start_ms: Option<u64> = None;

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
            match ch {
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
                // Ignore other control bytes (arrows, escape sequences, …).
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
    fn tracks_typing_start_time() {
        let cmds = reconstruct(&[(100, "a"), (140, "b"), (900, "c\r")]);
        assert_eq!(cmds[0].start_ms, 100);
        assert_eq!(cmds[0].enter_ms, 900);
    }
}
