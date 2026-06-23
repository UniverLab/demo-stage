---
title: Quickstart
description: Capture, record and export your first terminal demo in a few commands.
order: 3
---

# Quickstart

## Capture → record → export

```sh
# 1. Capture a live session. Run your demo, then type `demo stop` to finish
#    (`exit` / Ctrl-D still work too). capture normalizes automatically, so you get
#    both macro.raw.toml (the raw capture) and demo.toml (the clean score).
demo capture

# 2. (Optional) Validate the score.
demo check demo.toml

# 3. Execute the score to produce a recording (repeatable — re-run after changes).
demo record
#   → demo.cast

# 4. Render the recording (playback — never executes). Omit the format for all.
demo export html
#   → dist/<name>.html
```

For a tool you can't safely re-run (interactive, needs secrets), skip `record` and
render the live capture directly: `demo export html macro.raw.toml`.

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
demo check demo.toml
demo record                  # execute demo.toml → demo.cast
demo export gif              # render demo.cast → dist/hello.gif
demo export                  # …or all formats at once
```

See [the DSL reference](demo-toml.md) for every field and action.
