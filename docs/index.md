---
title: DemoStage
description: Demos as Code — record a terminal session, normalize it into a clean declarative score, and compile it to cast, html or gif.
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
demo record  ──>  macro.raw.toml  ──>  demo normalize  ──>  demo.toml
                                                                │
                          dist/  <──  demo export  <──  demo check
```

| Command | In | Out | Does |
|---|---|---|---|
| `record`    | TTY               | `macro.raw.toml` | Capture raw keystrokes, output and timing. |
| `normalize` | `macro.raw.toml`  | `demo.toml`      | Prune typos, humanize typing, trim idle. |
| `check`     | `demo.toml`       | exit 0/1         | Validate the score statically. |
| `export`    | `demo.toml`       | `dist/…`         | Compile to `cast`, `html`, `gif` (or `mp4`). |

You don't have to record: a `demo.toml` can be **authored by hand**, then checked
and exported.

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
