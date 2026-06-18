---
title: The smart normalizer
description: How demo normalize prunes typos, humanizes typing and trims idle time.
order: 6
---

# The smart normalizer

`demo normalize` is where a messy human recording becomes a clean score. Three
algorithms run over the raw event stream.

## 1. Backspace pruning

The raw input is replayed through a tiny line editor. Destructive edits are
applied and discarded, so only the *intended* command survives:

- `Backspace` / `Del` remove the previous character.
- `Ctrl-U` kills the whole line.
- `Ctrl-C` cancels the line.

```
typed:  g t i ⌫ ⌫ i t   ⏎      →   recorded: "git"
```

Blank lines (a bare `Enter`) are dropped.

## 2. Humanized typing

Each command becomes a `type` step with `human_salt = true`. At export, the
characters are written one at a time with a delay around `[typing].base_ms`, offset
by a bounded random **salt** (`±salt_ms`). The result reads like a fast human, not
an instant paste — and is **reproducible** when `[typing].seed` is set.

## 3. Idle trimming

Pauses between commands are kept (they feel natural) but **clamped**: too-short
gaps are widened to a readable minimum, long "think time" is capped. The trailing
idle that triggered the end of the recording is removed entirely, so the demo ends
crisply.

Timing is derived from the raw timestamps:

- between commands → the gap until the next command starts typing, clamped;
- after the last command → the time output kept arriving, capped.

## Result

The output is a `demo.toml` that already passes `demo check` and is ready to
`export` — or to hand-edit.
