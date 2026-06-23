---
title: Export targets
description: cast, html and gif are pure-Rust and offline; mp4 needs ffmpeg and browser panes need chromium.
order: 7
---

# Export targets

`demo export [fmt[,fmt…]] [score]` compiles a score. `cast`/`html`/`gif` are
pure-Rust and offline; `mp4` and multi-scene **browser panes** auto-provision
their tool (ffmpeg / Chromium) on first use. Pass several formats at once
(`demo export gif,mp4`), **omit the format to build them all** (`demo export`),
and use `--speed 2x` (or `3x`, `0.5x`) to retime the whole demo.

| Target | Output | External tool | Best for |
|---|---|---|---|
| `cast` | asciinema v2 (text) | — | Sharing/embedding a terminal recording. |
| `html` | self-contained player page | — | Dropping a demo onto a website. |
| `gif`  | animated GIF | — (terminal only) | READMEs, chat, anywhere images go. |
| `mp4`  | H.264 video | ffmpeg — **auto-fetched** | High-fidelity video, social. |
| browser panes | composited into gif/mp4 | Chromium — **auto-fetched** | PDF / web scenes beside the terminal. |

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

## mp4

Encoded with **ffmpeg**, which DemoStage provisions **tectonic-style**: if it
isn't on your `PATH`, the first `mp4` export notifies you and downloads a managed
static build into a cache, then reuses it. No manual install step. If the download
can't run (offline), you get a clear message and can install ffmpeg yourself.

## browser panes (multi-scene)

A score can place a `browser` pane next to the terminal (the spec's *Stage
Matrix*) — e.g. a PDF viewer or a live web preview. When exporting such a score to
`gif`/`mp4`, the **stage** runs the terminal in a PTY, drives a headless
**Chromium** to capture the `url` (scrolling per the `scroll` steps), and
composites both panes onto the canvas frame by frame.

Chromium is provisioned the same tectonic-style way as ffmpeg: a system Chrome is
used if present, otherwise `headless_chrome` downloads a managed build on first
use. (`cast`/`html` stay text-only and reject browser panes; use `gif`/`mp4` for
multi-scene.)

**Reveal on focus:** a browser pane is blank until the timeline `focus`es it, then
appears — so you can `focus` it right after a server comes up or a PDF compiles,
and it "opens" at that exact moment. (The focus time is recorded during the run.)

**Capture order:** the terminal runs to completion first, then each browser pane
is captured. So a browser pane must point at something still available at capture
time — a persistent file (a PDF, a rendered `preview.svg`/`.png`) or a server the
terminal leaves running (don't stop it mid-score). A step like `caption` overlays
a label on the canvas to narrate the demo.
