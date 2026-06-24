---
title: DemoStage
description: Demos as Code — record a terminal session, normalize it into a clean declarative score, and compile it to gif or mp4.
order: 1
---

# DemoStage

**DemoStage** turns terminal demonstrations into reproducible engineering. Instead
of capturing brittle, mutable pixels, it records a session as a sequence of
**events**, refines human imperfections into a clean **score** (`demo.toml`), and
compiles that score to several optimized formats.

A demo becomes a small, version-controlled, re-runnable artifact — diffable like
any other source file.

## The pipeline

```
demo capture  ──>  demo.rec  ──>  demo export  ──>  dist/
               (capture the real session)    (render — no re-run)

                   demo record  ──>  demo.rec   (optional: re-execute demo.toml
                                                   for a fresh, deterministic take)
```

| Command | In | Out | Does |
|---|---|---|---|
| `capture`   | TTY               | `demo.rec` (`--score` adds `demo.toml`, `--raw` the macro) | Capture a live session and save a faithful recording (forces a clean prompt). |
| `record`    | `demo.toml`       | `demo.rec`      | *(Optional)* Validate, then re-execute the score in a PTY → a fresh recording. |
| `export`    | `demo.rec`       | `dist/…`         | Render the recording to `gif` or `mp4`. Never executes. |

`demo export` works straight after `capture`. You don't have to capture, either: a
`demo.toml` can be **authored by hand**, then `record`ed and exported.

## Why it exists

Recording demos by hand is fiddly: typos, uneven pacing, dead air, and a prompt
that leaks your username and hostname. DemoStage fixes those mechanically — see
[the normalizer](normalizer.md) — so the result looks deliberate without being faked.

## Next

- [Installation](installation.md)
- [Quickstart](quickstart.md)
- [The `demo.toml` DSL](demo-toml.md)
- [Commands](commands.md)
- [The smart normalizer](normalizer.md)
- [Export targets](export-targets.md)
