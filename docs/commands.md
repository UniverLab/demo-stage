---
title: Commands
description: Reference for demo capture, open, record, check and export and their flags.
order: 5
---

# Commands

## `demo capture`

Capture a live interactive session into a raw macro, then normalize it into a
clean `demo.toml`. Needs a real terminal.

```sh
demo capture [-o macro.raw.toml] [-O demo.toml] [--no-normalize] [--debug] [--idle-timeout-ms 0] [--shell /bin/bash] [--into demo.toml]
```

- `-o, --output` — where to write the raw macro.
- `-O, --normalized-output` — where the automatic normalize writes the clean score
  (default `demo.toml`).
- `--no-normalize` — keep only the raw macro (no clean score). Useful when you'll
  render the capture directly — `demo export gif macro.raw.toml` — instead of
  re-executing it.
- `--debug` — write a timestamped diagnostic log next to the raw macro
  (`<output>.debug.log`): every input/output chunk in escaped + hex form (secret
  keystrokes are logged only as a byte count, never their value) and why the
  capture stopped. Use it when a capture behaves oddly (e.g. a wizard mis-reads a
  keystroke).
- `--idle-timeout-ms` — auto-stop after this long with no terminal output.
  **Defaults to `0` (disabled)** so a pause to think never cuts the capture
  short; set a positive value for an unattended capture. Any trailing idle is
  trimmed when normalizing.
- `--shell` — shell to run (defaults to `$SHELL`).
- `--into` — splice this capture into a hand-authored stage score's timeline
  instead of building a fresh single-pane score (advanced).

To add a **browser scene** (show a repo, a PDF, a localhost page) during a
capture, use [`demo open`](#demo-open) — no layout to author up front.

**Ending a capture.** Run `demo stop` (from inside the session or another shell in
the same directory) — the clean way to finish, even mid-wizard. (`exit` / Ctrl-D
still work; a positive `--idle-timeout-ms` also stops after that long with no
output.) The `demo stop` you type is dropped from the normalized score.

`capture` **normalizes automatically** when it finishes **and** saves a faithful
recording of the real session, so a single `demo capture` gives you
`macro.raw.toml`, a clean `demo.toml`, and a `demo.rec` — meaning **`demo export`
works straight after `capture`**, with no re-execution. (Run `demo record` later
only if you want to re-execute the score for a fresh take.)

Arrow keys and other special keys (Home/End, function keys) pressed inside an
interactive wizard are recorded as control sequences and **swallowed by the
normalizer** — they won't leak into the clean score as stray `[A`/`[B` text. (To
keep a wizard's exact navigation, render the raw capture directly with `export`.)

**Secrets are redacted.** When a program shows a secret prompt — a line ending in
`:` or `?` that mentions *password*, *passphrase*, *passcode*, *secret*, *token*,
*api key*, *access key*, *credential*, `[sudo]`, … — the keystrokes you type are
forwarded to the program but **never written to `macro.raw.toml`** (nor the
`--debug` log, which records only the byte count there). Notes:

- Secret prompts disable echo, so the secret isn't in the output either — but the
  detector is a **heuristic** (keyword + a `:`/`?` ending). A prompt without one of
  those keywords **won't** be caught, so **review the macro/score before sharing**,
  and prefer non-interactive bypasses (e.g. export `GITHUB_TOKEN` so ghScaff skips
  its vault passphrase — nothing is typed at all).
- A program that *prints* a secret to stdout (a token in its output) is **not**
  redacted — edit it out, since it lands in the recording and the gif/mp4.

## `demo open`

Reveal a **browser scene** in the running capture — show a repo page, a `file://`
PDF, a localhost server. Run it **inside** the capture (between commands) **or from
another terminal in the same directory** — the latter lets you trigger a reveal
live even while a full-screen TUI owns the captured shell.

```sh
demo open [url] [--replace | --split] [--when "<line>"] [--wizard]
```

Run it with no URL (on a terminal) — or with `--wizard` — for a small prompt that
asks the URL, the mode, and whether to reveal now or on a cue line. From a second
terminal the wizard's prompts stay out of the recording.

- `[url]` — what to show. Omit for the wizard.
- `--replace` (default) — the browser takes over the whole frame (a scene swap).
- `--split` — the browser sits beside the terminal (which keeps showing).
- `--when "<line>"` — **defer** the reveal until that substring appears in the
  terminal output. Arm it *before* running the program, so the scene opens on a cue
  line (e.g. when a build prints a URL) without you watching.

The reveal is baked into the recording and **composited at `export`** (the browser
is captured via headless Chromium). Example — open the repo when ghScaff prints it:

```sh
demo capture
demo open https://github.com/me/new-repo --when "github.com/me/new-repo"
ghscaff            # the scene opens when the URL appears
demo stop
demo export gif
```

## `demo stop`

End the in-progress capture. Run it **inside** a `demo capture` session, or from
another terminal in the same directory:

```sh
demo stop
```

The recorder writes a control file (`.demo-capture`) in its directory and exports
its path to the captured shell; `demo open`/`demo stop` append a command to it
(found by that env var, or by the cwd from another terminal). Outside a capture it
errors. The `demo stop` you type is dropped from the normalized score.

> **Normalizing** (prune typos, humanize typing, trim idle — see
> [the normalizer](normalizer.md)) is **not a separate command**: it runs
> automatically at the end of `demo capture`.

## `demo record`

Execute a demo score in a real PTY and save the result as a **recording** (a
`.rec` that `demo export` plays back). This is the repeatable step: re-run it
after the app changes and the recording refreshes.

```sh
demo record [demo.toml] [-o demo.rec]
```

- `[input]` — the score to execute; defaults to `demo.toml`.
- `-o, --output` — where to write the recording; defaults to `demo.rec`.

The score's commands **actually run**, so the recording reflects the real output.
A demo whose last command leaves a process in the foreground (a server, a REPL) is
killed after a short grace period rather than blocking; end such a step with
`Ctrl-C` or `terminate` to be clean. For a multi-pane stage, only the **terminal
pane** is executed and recorded; `export` composites the browser panes around it.

> Don't want to re-execute (interactive tool, needs secrets, has side effects)?
> Skip `record` and render the live capture directly: `demo export gif macro.raw.toml`.

## `demo check`

Statically validate a score. Exit `0` if valid, `1` otherwise; problems are listed
on stderr.

```sh
demo check [demo.toml]
```

Checks: canvas/fps sane, pane ids unique and inside the canvas, browser panes have
a `url`, and every timeline step targets the right kind of pane (e.g. you can't
`type` without a focused terminal, or `scroll` a terminal).

## `demo export`

Render a **recording** to one or more formats. Pure playback — it replays a
recording and **never executes** the demo.

```sh
demo export [fmt[,fmt…]] [demo.rec] [--speed 2x]
```

- `[formats]` — which formats to build, the first argument, **comma-separated**:
  `gif`, `mp4`, or `all` (see [export targets](export-targets.md)).
  Pass several at once (`demo export gif,mp4`) — and **omit it entirely to build
  every supported format** (`demo export` ≡ `demo export all`).
- `[input]` — the recording to render; defaults to `demo.rec`. Accepts a `.rec`
  from `demo record`, or a raw capture (`macro.raw.toml`) to render the live
  session directly (faithful playback — handles interactive tools, secrets and
  side effects that re-execution can't).
- `--speed` — retimes the recording: `2x`, `3x`, `0.5x` (a bare number works too).
  `1x` (the default) keeps the recorded pace.

Each format is written to its default path `<output_dir>/<name>.<ext>`.

For a **multi-pane stage**, `gif`/`mp4` composite the recorded terminal with its
browser panes — each captured via headless Chromium (auto-provisioned) and
revealed at the moment the timeline focuses it.
