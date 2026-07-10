---
title: Quickstart
description: Capture, record and export your first terminal demo in a few commands.
order: 3
---

# Quickstart

## Capture → record → export

```sh
# 1. Capture a live session. Run your demo, then `demo stop` to finish
#    (`exit` / Ctrl-D still work too). capture saves demo.rec (the faithful take)
#    and demo.toml (the editable score); pass --no-score for just the recording.
demo capture

# 2. Re-run the clean score to get a humanized recording (typing/idle normalized).
demo record            # validate + execute demo.toml → demo.rec

# 3. Render the recording (playback — never executes). Omit the format for all.
demo export gif
#   → dist/<name>.gif
```

### Interactive or side-effecting demos: export the capture directly

A demo you **can't re-run** — an interactive wizard, something that needs secrets
or creates real resources (e.g. a tool that makes a GitHub repo) — should skip
`record` (re-running would repeat or desync it) and render the faithful capture:

```sh
demo capture                 # run the demo, `demo stop`
demo export gif --force      # render the capture as-is (typing not re-humanized)
```

`export` refuses a faithful capture without `--force`, so the clean `record` path
is the default; `--force` is the explicit escape hatch for these cases.

## Or author by hand

You don't need to capture. Write a `demo.toml` directly:

```toml
[demo]
name = "hello"

[layout]
width = 800
height = 480

  [[layout.panes]]
  id = "main"
  type = "terminal"
  x = 0
  y = 0
  width = 800
  height = 480
  font_size = 16

[[timeline]]
action = "focus"
pane = "main"

[[timeline]]
action = "type"
text = "echo hello, universe"
human_salt = true

[[timeline]]
action = "keypress"
key = "enter"

[[timeline]]
action = "wait"
duration_ms = 800

[[timeline]]
action = "terminate"
```

```sh
demo record                  # validate + execute demo.toml → demo.rec
demo export gif              # render demo.rec → dist/hello.gif
demo export                  # …or all formats at once
```

See [the DSL reference](demo-toml.md) for every field and action.
