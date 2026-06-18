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

`cast`, `html` and `gif` work with **no external dependencies**.

- **`mp4`** → [ffmpeg](https://ffmpeg.org/). You don't have to install it: the
  first `mp4` export **auto-downloads a managed static ffmpeg** (tectonic-style)
  into a cache and reuses it. A system ffmpeg on your `PATH` is used if present.
- **browser panes** (PDF / web scenes) → a Chromium install. *Not supported yet*
  (the renderer is planned).

If a download can't run (offline), `demo export` fails with a clear message; the
pure-Rust targets keep working regardless.
