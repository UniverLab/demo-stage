---
title: Export targets
description: cast, html and gif are pure-Rust and offline; mp4 needs ffmpeg and browser panes need chromium.
order: 7
---

# Export targets

`demo export --target <fmt>` compiles a score. Three targets are pure-Rust and
fully offline; two need an external tool.

| Target | Output | External tool | Best for |
|---|---|---|---|
| `cast` | asciinema v2 (text) | — | Sharing/embedding a terminal recording. |
| `html` | self-contained player page | — | Dropping a demo onto a website. |
| `gif`  | animated GIF | — | READMEs, chat, anywhere images go. |
| `mp4`  | H.264 video | **ffmpeg** | High-fidelity video, social. |
| browser panes | (part of gif/mp4) | **chromium** | PDF / web scenes beside the terminal. |

## cast

asciinema v2: a JSON header plus timestamped output events. Tiny and text-only —
no pixels. Play it with any asciinema player.

## html

The `cast` embedded in a single HTML file that plays it with
[asciinema-player](https://docs.asciinema.org/). Ideal for the landing: a terminal
demo with zero video weight.

## gif

Pure-Rust rasterization. The captured output is replayed through a vt100 parser at
the score's `fps`, each frame is drawn with an **embedded monospace font**,
identical frames are deduped, and the result is encoded with the `gif` crate. No
ffmpeg. Covers printable ASCII and ANSI colors; exotic glyphs are skipped.

## mp4 and browser panes

`mp4` needs **ffmpeg** on the `PATH`. Multi-scene layouts with a `browser` pane
(the PDF viewer / web scene) need a **Chromium** install to render the web side.
When the tool is absent, `export` fails with a clear message — the offline targets
above are unaffected.
