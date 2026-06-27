---
title: Commands
description: Reference for demo capture, open, record, export, edit, source, scene, focus and doctor and their flags.
order: 5
---

# Commands

## `demo capture`

Capture a live interactive session and save it as a **recording** (`demo.rec`,
played back by `demo export`) plus the editable **score** (`demo.toml`, re-run by
`demo record`). Needs a real terminal.

```sh
demo capture [-r demo.rec] [-O demo.toml | --no-score] [--raw macro.raw.toml] [--no-normalize] [--prompt "<PS1>" | --keep-prompt] [--debug] [--idle-timeout-ms 0] [--shell /bin/bash] [--into demo.toml]
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

To add a **browser scene** (show a repo, a PDF, a localhost page) during a
capture, use [`demo open`](#demo-open) — no layout to author up front.

**Ending a capture.** Run `demo stop` (from inside the session or another shell in
the same directory) — the clean way to finish, even mid-wizard. (`exit` / Ctrl-D
still work; a positive `--idle-timeout-ms` also stops after that long with no
output.) The `demo stop` you type is dropped from the recording.

`capture` writes the normalized `demo.toml` (unless `--no-score`) for the clean
`demo record` → `demo export` path, **and** a **faithful** `demo.rec` of the real
session. For interactive tools, secrets and side effects (a `demo record` re-run
would repeat them), render that faithful recording directly with
`demo export --force`. The raw macro is dropped unless you pass `--raw`.

A `demo open` you run **inside** the capture (its typed command and wizard
prompts) is automatically excised from both the recording and the score, so the
meta-command never shows up in the finished demo.

Arrow keys and other special keys (Home/End, function keys) pressed inside an
interactive wizard are recorded as control sequences and **swallowed by the
normalizer** — they won't leak into the clean score as stray `[A`/`[B` text. (To
keep a wizard's exact navigation, render the raw capture directly with `export`.)

**Secrets are redacted, and re-asked at `record`.** When a program shows a secret
prompt — a line ending in `:` or `?` that mentions *password*, *passphrase*,
*passcode*, *secret*, *token*, *api key*, *access key*, *credential*, `[sudo]`, …
— the keystrokes you type are forwarded to the program but **never written
anywhere** (not the recording, the score, the raw macro, nor the `--debug` log,
which records only the byte count). Only the prompt's **label** is kept, so a later
`demo record` knows it needs that secret: it asks for each up front, keeps them
**only in memory** for the run, and types them at the matching prompt (verifying
the prompt is showing first, so a secret never lands in the wrong field). The
captured demo shows only the program's mask (`********`). Notes:

- The detector is a **heuristic** (keyword + a `:`/`?` ending). A prompt without one
  of those keywords **won't** be caught, so **review the recording before sharing**,
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
demo open [url] [--replace | --split] [--when "<line>" | --after] [--hold <ms> | --scroll] [--view] [--theme light|dark] [--wizard]
```

Run it with no URL (on a terminal) — or with `--wizard` — for a small prompt that
asks the URL, the theme, how to present it (static hold / scroll / interactive
view), the placement, and when to reveal. From a second terminal the wizard's
prompts stay out of the recording.

- `[url]` — what to show. Omit for the wizard.
- `--replace` (default) — the browser takes over the whole frame (a scene swap).
- `--split` — the browser sits beside the terminal (which keeps showing).
- `--when "<line>"` — **defer** the reveal until that substring appears in the
  terminal output. Arm it *before* running the program, so the scene opens on a cue
  line (e.g. when a build prints a URL) without you watching.
- `--after` — **defer** the reveal until the current foreground command finishes
  (its output goes quiet and the shell is back at the prompt). Arm it, then run
  your command; the scene opens the moment it returns. Doesn't need a cue line —
  handy for a wizard whose final output you can't predict. (Conflicts with
  `--when`.)
- `--hold <ms>` — hold the scene on screen this long after it opens, so a reveal
  near the end of the capture doesn't just flash by (defaults to a few seconds).
  **Mutually exclusive with `--scroll`.**
- `--scroll` — slowly pan the page down while the scene is shown, instead of a
  static hold. At `export` the page is scrolled across the window it's visible for.
- `--view` — open a **real (headed) browser** you drive yourself: navigate, click
  and scroll however you like; the session is recorded (~8 fps) **until you close
  the window**, then composited into the demo. It reveals immediately and takes the
  whole frame. The frames are recorded up front into `demo-scenes/`, so **no
  headless Chromium is needed at `export`** (handy when export runs on a host
  without a browser — keep `demo-scenes/` next to the `.rec`). Needs a graphical
  display (on WSL, WSLg). Conflicts with `--scroll`/`--hold`/`--when`/`--after`.
- `--theme light|dark` — emulate the browser's colour scheme (`prefers-color-scheme`)
  so theme-aware pages (GitHub, many docs sites) render the theme you want instead
  of defaulting to light. Applies to both headless reveals and `--view`. Omit for
  the page/browser default.

A reveal (other than `--view`) is composited at `export` via headless Chromium; a
`--view` scene plays back its recorded frames. Example — open the repo once ghScaff
finishes, and scroll it:

```sh
demo capture
demo open https://github.com/me/new-repo --after --scroll
ghscaff            # the scene opens when ghScaff returns to the prompt
demo stop
demo export gif --force   # ghScaff can't be re-run, so render the capture as-is
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
(`demo open`) and the `mp4` target need, and report exactly how to fix what's
missing on your platform. The core pipeline (`capture` → `record` → `export gif`)
is pure Rust and needs none of this.

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
- **display** — `demo open --view` (a headed browser) needs a graphical display
  (on WSL, WSLg). Headless reveals don't.

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

## `demo source`

Define a content source (terminal, browser) for scene composition. Run this
before `demo capture` to pre-define what goes into each scene.

```sh
demo source [ID] [-t terminal|browser] [-u URL] [-c light|dark] [--list] [--remove ID] [-s demo.toml]
```

- `[ID]` — unique identifier (e.g. "main", "google"). Prompted if omitted.
- `-t, --type` — source type: `terminal` or `browser`. Prompted if omitted.
- `-u, --url` — URL for browser sources (http, https, file://). Prompted if
  omitted for browser sources.
- `-c, --theme` — colour scheme for browser sources (`light`/`dark`). Prompted
  if omitted.
- `--list` — list existing sources and exit.
- `--remove <ID>` — remove a source by ID and exit.
- `-s, --score` — the demo score file to modify (default `demo.toml`).

**Interactive wizard:** Run with no arguments on a terminal for a guided prompt
that asks for the source ID, type, URL (for browsers), and theme.

**Examples:**

```sh
# Define a terminal source
demo source main --type terminal

# Define a browser source
demo source google --type browser --url "https://google.com" --theme dark

# List sources
demo source --list

# Remove a source
demo source --remove google
```

## `demo scene`

Define a scene composition from pre-defined sources. Scenes map layout strings
(e.g. "main+google") to concrete compositions. Run this before `demo capture`.

```sh
demo scene [ID] [-l LAYOUT] [--list] [--remove ID] [-s demo.toml]
```

- `[ID]` — unique identifier (e.g. "solo", "split"). Prompted if omitted.
- `-l, --layout` — layout string defining the composition of sources.
  Prompted if omitted.
- `--list` — list existing scenes and exit.
- `--remove <ID>` — remove a scene by ID and exit.
- `-s, --score` — the demo score file to modify (default `demo.toml`).

**Layout string syntax:**

- `"main"` — fullscreen single source
- `"main+google"` — 50/50 split
- `"main+google+github"` — thirds
- `"main*2+google"` — weighted (main gets 2/3)

**Interactive wizard:** Run with no arguments on a terminal for a guided prompt
that shows available sources and validates the layout string.

**Examples:**

```sh
# Define a scene
demo scene solo --layout "main"
demo scene split --layout "main+google"
demo scene full_github --layout "main+github"

# List scenes
demo scene --list
```

## `demo focus`

Switch focus to a scene during capture. Adds a `Step::Focus` entry to the
timeline. Supports deferred triggers (pattern match, command finish, timer).

```sh
demo focus [SCENE] [--when "<line>" | --after | --after-ms <MS>] [-s demo.toml]
```

- `[SCENE]` — scene ID to focus. Omit for an interactive picker.
- `--when "<line>"` — focus when this substring appears in terminal output.
- `--after` — focus after the current command finishes.
- `--after-ms <MS>` — focus after this many milliseconds from capture start.
- `--hold <MS>` — hold focus for this duration in milliseconds.
- `-s, --score` — the demo score file to modify (default `demo.toml`).

**Interactive wizard:** Run with no arguments on a terminal for a guided prompt
that shows available scenes and asks when to focus.

**Examples:**

```sh
# Focus immediately
demo focus split

# Focus when a pattern appears
demo focus github --when "Server started"

# Focus after command finishes
demo focus preview --after
```

> **Note:** Deferred triggers (`--when`, `--after`, `--after-ms`) are currently
> implemented as immediate focus. Full trigger semantics will be implemented in
> a future update.
