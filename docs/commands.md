---
title: Commands
description: Reference for demo capture, record, export, edit, doctor and in-capture / commands.
order: 5
---

# Commands

## `demo capture`

Capture a live interactive session and save it as a **recording** (`demo.rec`,
played back by `demo export`) plus the editable **score** (`demo.toml`, re-run by
`demo record`). Needs a real terminal.

```sh
demo capture [-r demo.rec] [-O demo.toml | --no-score] [--raw macro.raw.toml] [--no-normalize] [--prompt "<PS1>" | --keep-prompt] [--debug] [--idle-timeout-ms 0] [--shell /bin/bash] [--into demo.toml] [--font <name>]
```

- `-r, --rec` — where to write the recording (default `demo.rec`). It's a faithful
  take, so `demo export --force` renders it directly; the clean path is
  `demo record` (re-run the score) then `demo export`.
- `-O, --score <file>` — where to write the demo score (default `demo.toml`) — the
  "demo as code" source you can hand-edit or re-run with `demo record`.
- `--no-score` — leave only the recording (skip writing `demo.toml`).
- `--raw <file>` (`-o`) — **also** write the low-level raw macro, a debugging
  intermediate. Off by default.
- `--no-normalize` — skip the normalize pass, so no score is derived (the
  recording is faithful either way).
- `--prompt "<PS1>"` — force a clean prompt in the captured shell so the demo
  shows a tidy prompt instead of your real `user@host`. **On by default** with a
  built-in realistic prompt (`user@demo:~$`, green/blue); pass a value (bash `PS1`
  syntax) to customize. With neither `--prompt` nor `--keep-prompt`, a quick
  one-question wizard asks (clean / customize / keep yours) before recording.
- `--keep-prompt` — keep your shell's real prompt (don't force a clean one).
- `--debug` — write a timestamped diagnostic log next to the recording
  (`<rec>.debug.log`): every input/output chunk in escaped + hex form (secret
  keystrokes are logged only as a byte count, never their value) and why the
  capture stopped. Use it when a capture behaves oddly (e.g. a wizard mis-reads a
  keystroke).
- `--idle-timeout-ms` — auto-stop after this long with no terminal output.
  **Defaults to `0` (disabled)** so a pause to think never cuts the capture
  short; set a positive value for an unattended capture. Any trailing idle is
  trimmed when normalizing.
- `--shell` — shell to run (defaults to `$SHELL`).
- `--into` — splice this capture into a hand-authored stage score's timeline
  instead of building a fresh single-pane score (advanced); defaults `--score` to
  `demo.toml`.
- `--font` — font for the exported demo (`dejavu`, `jetbrains`, `ibm-plex`,
  `liberation`, `ubuntu`). Omit for a wizard prompt.

**Ending a capture.** Type `/stop` inside the session, or run `demo stop` from
another shell in the same directory. (`exit` / Ctrl-D still work; a positive
`--idle-timeout-ms` also stops after that long with no output.)

## In-capture commands

While a `demo capture` is running, you can type these commands directly in the
captured terminal. They are intercepted before reaching the shell and never
appear in the recording.

### `/stop`

End the capture. Same as running `demo stop` from another terminal.

```
/stop
```

### `/focus <scene>`

Switch focus to a scene. The scene must be defined in the score's `[scenes]`
table.

```
/focus split
/focus full_github
```

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
> Skip `record` and render the faithful capture directly with `demo export --force`
> (the `demo.rec` that `capture` already wrote).

The score `record` runs is validated first (canvas/fps sane, pane ids unique and
inside the canvas, browser panes have a `url`, every step targets the right kind of
pane); it refuses to run an invalid score.

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
  from `demo capture` (faithful — handles interactive tools, secrets and side
  effects that re-execution can't) or from `demo record` (a re-executed, humanized
  take), or a raw capture (`macro.raw.toml`, if you kept one with `--raw`) to
  render it directly.
- `--speed` — retimes the recording: `2x`, `3x`, `0.5x` (a bare number works too).
  `1x` (the default) keeps the recorded pace.
- `--force` — render a **faithful capture** as-is. By default `export` refuses a
  capture's `.rec` (its typing/idle aren't humanized) and points you at
  `demo record` for a clean re-take; pass `--force` to render the live capture
  directly anyway. This is the path for **interactive / side-effecting demos**
  (a wizard that creates a repo, a flow needing secrets) that a `demo record`
  re-run would repeat or desync — there, faithful + `--force` is the only option.

Each format is written to its default path `<output_dir>/<name>.<ext>`.

For a **multi-pane stage**, `gif`/`mp4` composite the recorded terminal with its
browser panes — each captured via headless Chromium (auto-provisioned) and
revealed at the moment the timeline focuses it.

## `demo doctor`

Check the environment for the optional dependencies that browser scenes
and the `mp4` target need, and report exactly how to fix what's missing on your
platform. The core pipeline (`capture` → `record` → `export gif`) is pure Rust
and needs none of this.

```sh
demo doctor [--fix]
```

It reports three checks:

- **chromium** — a browser the automation can drive. It flags the Ubuntu **snap**
  Chromium specifically: its sandbox blocks the remote-debug port, so it can't be
  driven (the `no available ports … for debugging` error). `demo` prefers a
  non-snap Google Chrome automatically.
- **ffmpeg** — needed for `mp4`. Missing is only a warning: `mp4` auto-downloads a
  managed copy on first use.
- **display** — headed browser sessions need a graphical display (on WSL, WSLg).
  Headless reveals don't.

`--fix` installs what's missing on apt-based Linux (a non-snap Google Chrome, and
ffmpeg) — it runs `sudo`, so it prompts in your terminal. On other platforms it
prints the exact `fix:` commands to run yourself.

## `demo edit`

```sh
demo edit [input]
```

Interactively edit timing and wait steps in a demo score. Opens a TUI with the
full timeline — navigate with **↑↓**, press **space** to edit a step, **enter**
to confirm you're done, **q** to quit.

Editing actions per step type:

- **Keep as-is** — no change
- **wait_for_quiet** — replace with a silence-based wait
- **wait_for_screen** — replace with a VT pattern match
- **wait_for_stdout** — replace with a raw output match
- **Change duration** — adjust the `duration_ms` of a `wait` step
- **Split/Edit text** — (Type steps only) rewrite the text or split by delimiter
- **Delete** — remove the step

`[input]` defaults to `demo.toml`.
