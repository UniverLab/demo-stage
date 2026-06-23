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
demo record  ──>  macro.raw.toml  +  demo.toml
                      │  (capture raw keystrokes + output + timing,
                      │   then normalize automatically)
                      ▼
                demo check  ──>  (ok · fail)
                      │  (validate the score statically)
                      ▼
                demo export  ──>  dist/  (cast · html · gif · mp4)
```

```sh
demo record                       # record a session, then type `demo stop` to finish
                                  # → macro.raw.toml + demo.toml (auto-normalized)
demo check demo.toml              # static validation
demo export html                  # → dist/<name>.html
demo export                       # no args → every format (cast, html, gif, mp4)
demo export gif,mp4 --speed 2x        # several at once, retimed 2× faster
```

`record` runs `normalize` for you when it finishes (pass `--no-normalize` to skip
it, then run `demo normalize` yourself). You can also skip recording entirely and
**author `demo.toml` by hand**, then `check` + `export`.

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
| `cast` | asciinema v2 (text) | — (pure Rust) |
| `html` | self-contained player page | — (pure Rust) |
| `gif`  | animated GIF (rasterized) | — (pure Rust, embedded font) |
| `mp4`  | H.264 video | ffmpeg — **auto-fetched on first use** |
| browser panes (PDF/web) | composited into gif/mp4 | Chromium — **auto-fetched on first use** |

`cast` / `html` / `gif` work fully offline. `mp4` and multi-scene **browser
panes** provision their tool **tectonic-style** — the first export downloads a
managed ffmpeg / Chromium into a cache (a system install is used if present).

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
