---
title: Export targets
description: cast, html and gif are pure-Rust and offline; mp4 needs ffmpeg and browser panes need chromium.
order: 7
---

# Export targets

`demo export --target <fmt>` compiles a score. Three targets are pure-Rust and
fully offline; `mp4` auto-provisions its tool on first use; browser panes are not
supported yet.

| Target | Output | External tool | Best for |
|---|---|---|---|
| `cast` | asciinema v2 (text) | — | Sharing/embedding a terminal recording. |
| `html` | self-contained player page | — | Dropping a demo onto a website. |
| `gif`  | animated GIF | — | READMEs, chat, anywhere images go. |
| `mp4`  | H.264 video | ffmpeg — **auto-fetched** | High-fidelity video, social. |
| browser panes | (planned) | chromium — *planned* | PDF / web scenes beside the terminal. |

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

## browser panes (planned)

Multi-scene layouts with a `browser` pane (the PDF viewer / web scene) are declared
in the DSL and validated by `check`, but the renderer isn't built yet — it needs a
headless **Chromium** (no pure-Rust option exists) plus a frame compositor. Until
then, exporting a score with a browser pane reports a clear "not supported yet"
error; the same auto-provisioning approach as ffmpeg is planned for Chromium.
