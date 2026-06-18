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

`cast`, `html` and `gif` work with no external dependencies. Two targets need a
tool on your `PATH`:

- **`mp4`** → [ffmpeg](https://ffmpeg.org/)
- **browser panes** (PDF / web scenes) → a Chromium/Chrome install

If the tool is missing, `demo export` fails with a clear message; the offline
targets keep working.
