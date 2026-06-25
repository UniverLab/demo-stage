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

## Install

```sh
cargo install --path .          # from this repo
# or, once published:
# curl -fsSL https://get.univerlab.org/demo-stage | sh
```

## The loop

```
demo capture  ──>  demo.toml ──> demo record ──> demo.rec ──> demo export ──> dist/
                  (the editable score)  (re-run clean)     (render — never executes)
                       └──────────────────────────────────────────┐
                       demo.rec (faithful)  ──>  demo export --force ┘  (no re-run)
```

```sh
demo capture                      # run the demo, then type `demo stop` to finish
                                  # → demo.toml (editable score) + demo.rec (faithful take)
demo record                       # re-run demo.toml for a clean, humanized demo.rec
demo export                       # no args → every format (gif, mp4)
demo export gif,mp4 --speed 2x        # several at once, retimed 2× faster
```

**Capture** runs the demo live, writing an editable `demo.toml` score and a
faithful `demo.rec` of the real session; it forces a clean prompt so your real
`user@host` never shows. **Record** re-executes `demo.toml` for a clean, humanized
recording — the default render path. **Export** is pure playback: it renders a
recording and never executes anything.

**Can't re-run it?** Interactive tools, flows needing secrets, or demos that create
real resources (a wizard that makes a repo) would be repeated or desynced by
`record` — so skip it and render the faithful capture directly with
`demo export --force` (typing isn't re-humanized, but nothing re-executes). Export
refuses a faithful capture without `--force`, keeping the clean path the default.

**Multi-scene.** To show a **browser scene** (a repo page, a PDF, a localhost
server) during a capture, run `demo open <url>` — from the captured shell or
another terminal in the same directory (handy mid-TUI). Reveal it now, when a cue
line appears (`--when "<line>"`), or when the running command finishes (`--after`);
`--scroll` pans the page. The scene is composited into the gif/mp4.

## What makes it different

- **Backspace pruning** — typos corrected while recording never reach the demo.
- **Humanized typing** — a seeded, bounded jitter so playback reads like a fast
  human, not a robotic paste (reproducible with `[typing].seed`).
- **Idle trimming** — dead time between commands is clamped; the trailing idle that
  stops the recording is dropped.
- **Clean prompt** — capture forces a realistic generic prompt (`user@demo:~$`),
  so demos never leak your real `user@host` (`--keep-prompt`/`--prompt` to override).

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

## Documentation

See [`docs/`](docs/) — overview, the `demo.toml` DSL, the commands, the normalizer,
and the export targets. (Published on the UniverLab site at `/demo-stage/docs`.)

## Development

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

## License

MIT — see [`LICENSE`](LICENSE). The embedded font has its own permissive license,
see [`assets/FONT-LICENSE.md`](assets/FONT-LICENSE.md).
