```
██████  ███████ ███    ███  ██████      ███████ ████████  █████   ██████  ███████
██   ██ ██      ████  ████ ██    ██     ██         ██    ██   ██ ██       ██
██   ██ █████   ██ ████ ██ ██    ██     ███████    ██    ███████ ██   ███ █████
██   ██ ██      ██  ██  ██ ██    ██          ██    ██    ██   ██ ██    ██ ██
██████  ███████ ██      ██  ██████      ███████    ██    ██   ██  ██████  ███████
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

---

## Install

```sh
cargo install --path .          # from this repo
# or, once published:
# curl -fsSL https://get.univerlab.org/demo-stage | sh
```

## The loop

```
demo capture  ──>  demo.rec   (capture the real session live — one file)
                       │       (add --score for demo.toml, --raw for the macro)
                       ▼
demo export   ──>  dist/  (gif · mp4)            (render — never executes)

                   demo record  ──>  demo.rec   (optional: re-run demo.toml for a
                                                   fresh take when the app changes)
```

```sh
demo capture                      # capture a session, then type `demo stop` to finish
                                  # → demo.rec  (the one file export needs)
demo export                       # no args → every format (gif, mp4)
demo export gif,mp4 --speed 2x        # several at once, retimed 2× faster
```

**Capture** runs the demo live and saves a faithful recording (`demo.rec`) of the
real session — so **`demo export` works straight after `capture`**, with no
re-execution, and it forces a clean prompt so your real `user@host` never shows.
**Export** is pure playback: it renders a recording and never executes anything
(great for interactive tools, secrets, side effects).

**Record** is optional: it re-executes the clean `demo.toml` to refresh `demo.rec` —
use it for deterministic demos you want to keep current as the app changes. Capture
writes `demo.toml` only with `--score`; you can also **author it by hand**, then
`record` + `export`.

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
Chromium into a cache (a system install is used if present).

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
