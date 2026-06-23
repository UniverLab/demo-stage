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
demo capture  ──>  macro.raw.toml + demo.toml + demo.rec   (capture live, normalize,
                       │                                       record the real session)
                       ▼
demo export   ──>  dist/  (html · gif · mp4)                 (render — never executes)

                   demo record  ──>  demo.rec   (optional: re-run the score for a
                                                   fresh take when the app changes)
```

```sh
demo capture                      # capture a session, then type `demo stop` to finish
                                  # → macro.raw.toml + demo.toml + demo.rec
demo export                       # no args → every format (html, gif, mp4)
demo export gif,mp4 --speed 2x        # several at once, retimed 2× faster
```

**Capture** runs the demo live, **normalizes automatically**, and saves a faithful
recording (`demo.rec`) of the real session — so **`demo export` works straight after
`capture`**, with no re-execution. **Export** is pure playback: it renders a recording
and never executes anything (great for interactive tools, secrets, side effects).

**Record** is optional: it re-executes the clean `demo.toml` to refresh `demo.rec` —
use it for deterministic demos you want to keep current as the app changes. You can
also **author `demo.toml` by hand**, then `record` + `export`.

## What makes it different

- **Backspace pruning** — typos corrected while recording never reach the demo.
- **Humanized typing** — a seeded, bounded jitter so playback reads like a fast
  human, not a robotic paste (reproducible with `[typing].seed`).
- **Idle trimming** — dead time between commands is clamped; the trailing idle that
  stops the recording is dropped.
- **Clean prompt** — export forces `PS1='$ '`, so demos never leak `user@host`.

## Export targets

| Target | Output | Needs |
|---|---|---|
| `html` | self-contained player page (text) | — (pure Rust) |
| `gif`  | animated GIF (rasterized) | — (pure Rust, embedded font) |
| `mp4`  | H.264 video | ffmpeg — **auto-fetched on first use** |
| browser panes (PDF/web) | composited into gif/mp4 | Chromium — **auto-fetched on first use** |

`html` / `gif` work fully offline. `mp4` and multi-scene **browser panes**
provision their tool **tectonic-style** — the first export downloads a managed
ffmpeg / Chromium into a cache (a system install is used if present).

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
