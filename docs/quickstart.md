---
title: Quickstart
description: Record, normalize, check and export your first terminal demo in four commands.
order: 3
---

# Quickstart

## Record → normalize → export

```sh
# 1. Record an interactive session. Type your demo, then Ctrl-D (or `exit`).
demo record -o macro.raw.toml

# 2. Refine it into a clean, declarative score.
demo normalize macro.raw.toml -o demo.toml

# 3. Validate.
demo check demo.toml

# 4. Compile to a self-contained HTML player (great for a website).
demo export html demo.toml
#   → dist/<name>.html
```

## Or author by hand

You don't need to record. Write a `demo.toml` directly:

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
demo export gif demo.toml    # → dist/hello.gif
demo export cast demo.toml   # → dist/hello.cast
```

See [the DSL reference](demo-toml.md) for every field and action.
