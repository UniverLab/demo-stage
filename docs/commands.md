---
title: Commands
description: Reference for demo record, normalize, check and export and their flags.
order: 5
---

# Commands

## `demo record`

Capture an interactive session into a raw macro. Needs a real terminal.

```sh
demo record [-o macro.raw.toml] [--idle-timeout-ms 0] [--shell /bin/bash]
```

- `-o, --output` — where to write the raw macro.
- `--idle-timeout-ms` — auto-stop after this long with no terminal output.
  **Defaults to `0` (disabled)** so a pause to think never cuts the recording
  short; set a positive value for an unattended capture. Any trailing idle is
  trimmed by `normalize`.
- `--shell` — shell to run (defaults to `$SHELL`).

Recording ends when the shell exits (`exit` / Ctrl-D) — or, if you set a positive
`--idle-timeout-ms`, after that long with no output.

**Secrets are redacted.** When a program shows a password/passphrase prompt (a line
ending in `:` or `?` that mentions *password*, *passphrase*, *passcode*, *secret*,
`[sudo]`, …), the keystrokes you type are forwarded to the program but **never
written to `macro.raw.toml`**. Notes:

- Password prompts disable echo, so the secret isn't in the output either — but the
  detector is a heuristic, so **review the macro/score before sharing**, and prefer
  non-interactive bypasses for secret flows (e.g. export `GITHUB_TOKEN` so ghScaff
  skips its vault passphrase).
- A program that *prints* a secret to stdout (a token in its output) is not
  redacted — edit it out of the score.

## `demo normalize`

Refine a raw macro into a clean score.

```sh
demo normalize [macro.raw.toml] [-o demo.toml] [--seed N] [--typing-ms 80] [--salt-ms 15]
```

- `--seed` — make the humanized typing reproducible.
- `--typing-ms` / `--salt-ms` — written into the score's `[typing]` table.

See [the normalizer](normalizer.md) for what it does.

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

Compile a score to a target format.

```sh
demo export [demo.toml] --target <cast|html|gif|mp4> [-o PATH]
```

- `--target` — output format (see [export targets](export-targets.md)).
- `-o, --output` — output path; defaults to `<output_dir>/<name>.<ext>`.

Export runs the timeline in a real PTY with a clean prompt, capturing the output —
so the typed commands actually execute.
