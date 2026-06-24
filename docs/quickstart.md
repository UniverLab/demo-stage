---
title: Quickstart
description: Capture, record and export your first terminal demo in a few commands.
order: 3
---

# Quickstart

## Capture → export

```sh
# 1. Capture a live session. Run your demo, then type `demo stop` to finish
#    (`exit` / Ctrl-D still work too). capture saves demo.rec (export plays it) and
#    demo.toml (record re-runs it); pass --no-score for just the recording.
demo capture

# 2. Render the recording (playback — never executes). Omit the format for all.
demo export gif
#   → dist/<name>.gif
```

That's it — `export` plays back the recording `capture` just made, so it works for
interactive tools, secrets and side effects (no re-running).

### Optional: refresh the recording by re-running

For a deterministic demo you want to keep current as the app changes, re-execute
the `demo.toml` capture wrote (or one you authored by hand) to produce a fresh
`demo.rec`:

```sh
demo record            # validate + execute demo.toml → demo.rec
demo export            # render every format
```

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
