```
     █████                                    █████████   █████                               
    ░░███                                    ███░░░░░███ ░░███                                
  ███████   ██████  █████████████    ██████ ░███    ░░░  ███████    ██████    ███████  ██████ 
 ███░░███  ███░░███░░███░░███░░███  ███░░███░░█████████ ░░░███░    ░░░░░███  ███░░███ ███░░███
░███ ░███ ░███████  ░███ ░███ ░███ ░███ ░███ ░░░░░░░░███  ░███      ███████ ░███ ░███░███████ 
░███ ░███ ░███░░░   ░███ ░███ ░███ ░███ ░███ ███    ░███  ░███ ███ ███░░███ ░███ ░███░███░░░  
░░████████░░██████  █████░███ █████░░██████ ░░█████████   ░░█████ ░░████████░░███████░░██████ 
 ░░░░░░░░  ░░░░░░  ░░░░░ ░░░ ░░░░░  ░░░░░░   ░░░░░░░░░     ░░░░░   ░░░░░░░░  ░░░░░███ ░░░░░░  
                                                                             ███ ░███         
                                                                            ░░██████          
                                                                             ░░░░░░           
```

<p align="center">
  <a href="https://github.com/UniverLab/demo-stage/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/UniverLab/demo-stage/ci.yml?branch=main&style=for-the-badge&label=CI" alt="CI"/></a>
  <img src="https://img.shields.io/badge/Status-Alpha-E6C84A?style=for-the-badge" alt="Status"/>
  <img src="https://img.shields.io/badge/Demos-as%20Code-8e8ff0?style=for-the-badge" alt="Demos as Code"/>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-2E8B57?style=for-the-badge" alt="License"/></a>
</p>

**DemoStage** turns terminal demos into reproducible engineering — *Demos as Code*.
Instead of capturing brittle pixels, it records a session as **events**, normalizes
human imperfections into a clean **score** (`demo.toml`), and compiles that score to
several formats. One demo, version-controlled, re-runnable, diffable.

<p align="center">
  <img src="demo/dist/demo.gif" alt="A ghScaff wizard demo captured and rendered with DemoStage" width="800"/>
</p>

<p align="center"><em>An interactive ghScaff wizard — captured, normalized and rendered with DemoStage, browser scene composited in.</em></p>

---

## Features

- **🎬 Event-based capture** — Records keystrokes, commands, and window events as structured data, not pixels.
- **✏️ Editable score** — The `demo.toml` score is human-readable TOML you can prune, reorder, or retime by hand.
- **🧹 Backspace pruning** — Typos corrected while recording never reach the demo.
- **⌨️ Humanized typing** — Seeded, bounded jitter so playback reads like a fast human, not robotic paste.
- **🔇 Idle trimming** — Dead time between commands is clamped; trailing idle is dropped.
- **🔒 Clean prompt** — Capture forces a realistic generic prompt (`user@demo:~$`), so demos never leak your real `user@host`.
- **🎬 Multi-scene** — Composite browser scenes (repo pages, PDFs, localhost) beside the terminal.
- **🔤 Font selection** — Choose from 5 bundled monospace fonts during capture (DejaVu Sans Mono default, IBM Plex Mono, JetBrains Mono, Liberation Mono, Ubuntu Mono).
- **⚡ In-capture commands** — Type `/stop` to finish capture or `/focus <scene>` to switch layouts mid-session.
- **📦 Single binary** — Pure Rust, no Node.js, no Python, no runtime dependencies.

---

## Install

```sh
cargo install --path .          # from this repo
# or, once published:
# curl -fsSL https://get.univerlab.org/demo-stage | sh
```

### Via cargo

```bash
cargo install demo-stage
```

### Uninstall

```bash
rm -f ~/.local/bin/demo-stage
```

---

## Quick Start

```sh
demo capture                      # run the demo, then type `/stop` to finish
                                  # → demo.toml (editable score) + demo.rec (faithful take)
demo record                       # re-run demo.toml for a clean, humanized demo.rec
demo export                       # no args → every format (gif, mp4)
demo export gif,mp4 --speed 2x        # several at once, retimed 2× faster
```

## Documentation

See [`docs/`](docs/) — overview, the `demo.toml` DSL, the commands, the normalizer,
and the export targets. (Published on the UniverLab site at `/demo-stage/docs`.)

---

## The loop

```
demo capture  ──>  demo.toml ──> demo record ──> demo.rec ──> demo export ──> dist/
                  (the editable score)  (re-run clean)     (render — never executes)
                       └──────────────────────────────────────────┐
                       demo.rec (faithful)  ──>  demo export --force ┘  (no re-run)
```

**Capture** runs the demo live, writing an editable `demo.toml` score and a
faithful `demo.rec` of the real session; it forces a clean prompt so your real
`user@host` never shows. Sources (terminals, browsers) and scene layouts are
configured during the capture wizard or defined in the score beforehand.

**Record** re-executes `demo.toml` for a clean, humanized recording — the default
render path. **Export** is pure playback: it renders a recording and never executes
anything.

**Can't re-run it?** Interactive tools, flows needing secrets, or demos that create
real resources would be repeated or desynced by `record` — so skip it and render
the faithful capture directly with `demo export --force` (typing isn't re-humanized,
but nothing re-executes). Export refuses a faithful capture without `--force`, keeping
the clean path the default.

---

## Commands

### Top-level

| Command | Description |
|---|---|
| `demo capture` | Live capture: record the session, auto-normalize into a clean score and faithful `.rec` |
| `demo record` | Re-execute `demo.toml` cleanly, producing a humanized recording |
| `demo export` | Pure playback: render to gif or mp4 (no re-execution, ffmpeg/chromium auto-provisioned) |
| `demo edit` | Interactively edit timing and wait steps in a demo score |
| `demo doctor` | Verify the environment and install missing tools |

### In-capture

Type these in the terminal during a `demo capture` session:

| Command | Description |
|---|---|
| `/stop` | Stop the capture |
| `/focus <scene>` | Switch the layout to a different scene |

---

## What makes it different

- **Backspace pruning** — typos corrected while recording never reach the demo.
- **Humanized typing** — a seeded, bounded jitter so playback reads like a fast
  human, not a robotic paste (reproducible with `[typing].seed`).
- **Idle trimming** — dead time between commands is clamped; the trailing idle that
  stops the recording is dropped.
- **Clean prompt** — capture forces a realistic generic prompt (`user@demo:~$`),
  so demos never leak your real `user@host` (`--keep-prompt`/`--prompt` to override).

---

## Export targets

| Target | Output | Best for | Needs |
|---|---|---|---|
| `gif`  | animated GIF (rasterized) | READMEs, chat, anywhere `<img>` works | — (pure Rust, embedded font) |
| `mp4`  | H.264 video | landings / the web (`<video>`) | ffmpeg — **auto-fetched on first use** |
| browser panes (PDF/web) | composited into gif/mp4 | a scene beside the terminal | Chromium — **auto-fetched on first use** |

`gif` works fully offline. `mp4` and multi-scene **browser panes** provision
their tool **tectonic-style** — the first export downloads a managed ffmpeg /
Chromium into a cache (a system install is used if present). Run **`demo doctor`**
to check these and get platform-specific fixes (`--fix` installs them on apt-based
Linux; it also flags the snap Chromium, which can't be driven headless).

---

## Development

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

---

## License

MIT — see [`LICENSE`](LICENSE). The embedded font has its own permissive license,
see [`assets/FONT-LICENSE.md`](assets/FONT-LICENSE.md).

---

An experiment of [UniverLab](https://github.com/UniverLab) — an open computational laboratory.
Made with ❤️ by [JheisonMB](https://github.com/JheisonMB)
