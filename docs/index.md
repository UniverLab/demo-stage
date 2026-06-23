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
demo capture  ──>  macro.raw.toml + demo.toml   (capture live, normalize automatically)
                                          │
              dist/  <──  demo export  <──  demo record  <──  (demo check)
              (render)        (re-execute → demo.cast)
```

| Command | In | Out | Does |
|---|---|---|---|
| `capture`   | TTY               | `macro.raw.toml` + `demo.toml` | Capture a live session, then normalize (prune typos, humanize typing, trim idle). |
| `check`     | `demo.toml`       | exit 0/1         | Validate the score statically. |
| `record`    | `demo.toml`       | `demo.cast`      | Execute the score in a PTY → a recording. Repeatable. |
| `export`    | `demo.cast`       | `dist/…`         | Render the recording to `cast`, `html`, `gif`, `mp4`. Never executes. |

You don't have to capture: a `demo.toml` can be **authored by hand**, then
`record`ed and exported. And for a tool you can't safely re-run, render the live
capture directly with `demo export gif macro.raw.toml`.

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
