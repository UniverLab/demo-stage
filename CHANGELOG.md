# Changelog

All notable changes to DemoStage are documented here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/); versions follow SemVer.

## [Unreleased]

### Added
- **Pipeline**: `demo capture` (live PTY capture → a faithful `demo.rec`),
  `demo open` / `demo stop` (control a running capture), `demo record` (re-execute a
  `demo.toml` score → a fresh `demo.rec`), `demo export` (playback render — never
  executes). Normalization (backspace pruning, humanized typing, idle trimming) runs
  inside `capture`/`record`, not as a separate command.
- **DSL**: `demo.toml` score and `macro.raw.toml` capture (`[demo]` incl. a
  configurable `prompt`, `[env]`, `[typing]`, `[layout]` + panes with `line_height`,
  `[[timeline]]` actions: focus/type/keypress/wait/wait_for_stdout/scroll/caption/
  terminate).
- **Faithful capture**: `capture` saves a `.rec` of the *real* session — handling
  interactive tools, secrets and side effects that re-execution can't — alongside
  the editable `demo.toml`. `demo export` renders a faithful capture only with
  `--force` (so the clean `demo record` path is the default); `--raw` also keeps the
  low-level macro, `--no-score` drops `demo.toml`.
- **Prompt wizard**: with neither `--prompt` nor `--keep-prompt`, `demo capture`
  asks a one-question wizard (clean / customize / keep yours) before recording.
- **`demo open`** reveals a **browser scene** (repo page, `file://` PDF, localhost)
  during a capture — from the captured shell *or another terminal in the same
  directory* (via a `.demo-capture` control file), so it works mid-TUI. Reveal now,
  `--when "<line>"` (on a cue line), or `--after` (when the running command
  finishes); `--replace`/`--split` placement; `--hold <ms>` and `--scroll` to keep
  the scene up and pan it. A small wizard runs when no URL is given. An in-session
  `demo open` (its echo + wizard) is excised from the recording and the score.
- **Export targets**: `gif` — pure Rust rasterizer (vt100 + embedded DejaVu Sans
  Mono, with procedurally-drawn block/box-drawing glyphs so banners/TUIs stay solid);
  `mp4` — H.264 via ffmpeg. `--speed` retimes playback.
- **Multi-scene stage**: terminal + `browser` panes composited onto a shared canvas,
  driving headless Chromium for browser panes; reveals are reproduced by `demo record`
  too. Browser launch disables the sandbox for headless WSL/CI hosts.
- **Tectonic-style provisioning**: `mp4` auto-fetches a managed ffmpeg and browser
  panes auto-fetch Chromium on first use; a system install is preferred if present.
- **Clean prompt**: `capture` forces a realistic generic prompt (`user@demo:~$`,
  green/blue) by default so demos never leak your real `user@host`; `--keep-prompt`
  or `--prompt "<PS1>"` to override.
- **Captions**: a `caption` timeline action overlays on-canvas step labels (gif/mp4).
- **Secret redaction**: `capture` detects password/passphrase/token/… prompts and
  never records the keystrokes typed at them (forwarded to the program only); the
  `--debug` log stores only a byte count there.
- **`[env].requires`**: declare env vars export needs (provided by the runner, not
  stored); `demo record` validation fails when one is unset — reproducible
  secret-gated demos.

### Changed
- **`demo export` takes the target as its first argument**: `demo export gif [rec]`
  instead of `demo export [rec] --target gif`; comma-separated targets and `all`.
- **Removed `demo check`, `demo normalize` and `demo prepare` as standalone
  commands**, and the `cast`/`html` export targets (and the asciinema dependency):
  the intermediate is now a `.rec`, and validation runs as part of `demo record`.

### Fixed
- **Captures of interactive tools no longer desync.** `export` used to re-execute the
  wizard (cancelling ghScaff, etc.); it is now pure playback of the faithful `.rec`.
- **`capture` no longer cuts off on a pause.** The idle-timeout defaults to `0`
  (disabled); recording stops on `demo stop`/`exit`/Ctrl-D (or a positive
  `--idle-timeout-ms` you opt into).
- **`demo export` can no longer hang forever.** A demo whose last command left a
  process in the foreground made teardown block indefinitely; teardown is now bounded
  (grace period, then kill) and the capture thread is drained with a cap.

### Notes
- `gif` and the core pipeline are fully offline. The Chromium screenshot path
  (browser panes) is exercised on machines with Chromium available, not in the
  restricted sandbox.
