---
title: The demo.toml DSL
description: Every table and timeline action in a demo score — demo, env, typing, layout, panes and the timeline.
order: 4
---

# The `demo.toml` DSL

A score is a TOML document with four areas: metadata, the (optional) sandbox env,
the typing parameters, the canvas layout, and the timeline.

## `[demo]`

```toml
[demo]
name = "my-demo"          # used for the output filename
output_dir = "./dist"     # default
```

## `[env]` (optional)

A sandbox for an isolated, reproducible run. Scripts run **before** the clock
starts (setup) and **after** the timeline (teardown), so their output is not
captured.

```toml
[env]
isolated = true
setup_script = "mkdir -p /tmp/demo-sandbox && cd /tmp/demo-sandbox"
teardown_script = "rm -rf /tmp/demo-sandbox"
```

## `[typing]` (optional)

Controls the humanized per-character jitter applied to `type` steps that set
`human_salt = true`.

```toml
[typing]
base_ms = 80     # base speed, ms per character
salt_ms = 15     # max jitter ±, ms
seed = 42        # omit for a random feel; set for reproducible output
```

## `[layout]` and `[[layout.panes]]`

The canvas (pixels) and the scenes placed on it.

```toml
[layout]
width = 1920
height = 1080
fps = 15
background = "#0b0f14"

  [[layout.panes]]
  id = "console"        # unique id, referenced by the timeline
  type = "terminal"     # "terminal" | "browser"
  x = 0
  y = 0
  width = 960
  height = 1080
  font_family = "monospace"   # terminal only
  font_size = 16              # terminal only

  [[layout.panes]]
  id = "preview"
  type = "browser"
  x = 960
  y = 0
  width = 960
  height = 1080
  url = "file:///tmp/demo-sandbox/output.pdf"   # browser requires a url
```

`check` verifies pane ids are unique, panes fit the canvas, and browser panes have
a `url`.

## `[[timeline]]`

Steps share one timeline, each tagged by `action`:

| `action` | Fields | Meaning |
|---|---|---|
| `focus` | `pane` | Make a pane the active input target. |
| `type` | `text`, `human_salt?` | Type into the focused terminal. |
| `keypress` | `key` | Press a named key — `enter`, `tab`, `esc`, `up`, `ctrl+c`, … |
| `wait` | `duration_ms` | Hold for a fixed time. |
| `caption` | `text` | Show an on-canvas step label (empty `text` clears it). Rendered on `gif`/`mp4`; ignored by text-only `cast`/`html`. |
| `wait_for_stdout` | `match`, `pane?` | Block until a substring appears. |
| `scroll` | `direction`, `velocity?`, `duration_ms`, `pane?` | Scroll a browser pane. |
| `terminate` | — | End the demo. |

```toml
[[timeline]]
action = "focus"
pane = "console"

[[timeline]]
action = "type"
text = "cargo build --release"
human_salt = true

[[timeline]]
action = "keypress"
key = "enter"

[[timeline]]
action = "wait_for_stdout"
match = "Finished"

[[timeline]]
action = "terminate"
```
