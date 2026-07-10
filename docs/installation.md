---
title: Installation
description: Install the demo binary from source, with cargo, or the one-line installer.
order: 2
---

# Installation

DemoStage ships a single binary, `demo`.

## From source

```sh
git clone https://github.com/UniverLab/demo-stage
cd demo-stage
cargo install --path .
```

## With cargo (once published)

```sh
cargo install demo-stage
```

## One-line installer (once the domain is live)

```sh
curl -fsSL https://get.univerlab.org/demo-stage | sh
```

## Optional external tools

`gif` works with **no external dependencies**.

- **`mp4`** → [ffmpeg](https://ffmpeg.org/). You don't have to install it: the
  first `mp4` export **auto-downloads a managed static ffmpeg** (tectonic-style)
  into a cache and reuses it. A system ffmpeg on your `PATH` is used if present.
- **browser panes** (`demo open`, composited into `gif`/`mp4`) → Chromium/Chrome.
  A system browser is used if present, otherwise a managed one is fetched.

Run **`demo doctor`** to check all of this and get platform-specific fixes
(`demo doctor --fix` installs them on apt-based Linux). One gotcha it flags: on
Ubuntu/WSL the default Chromium is a **snap**, whose sandbox blocks the debug port
the automation needs — install a non-snap Google Chrome (picked automatically), or
just run `demo doctor --fix`.

If a download can't run (offline), `demo export` fails with a clear message; the
pure-Rust targets keep working regardless.
