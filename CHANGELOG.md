# Changelog

All notable changes to DemoStage are documented here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/); versions follow SemVer.

## [Unreleased]

### Added
- **Pipeline**: `demo record` (PTY capture), `demo normalize` (backspace pruning,
  humanized typing, idle trimming), `demo check` (static validation), `demo export`.
- **DSL**: `demo.toml` score and `macro.raw.toml` capture (`[demo]`, `[env]`,
  `[typing]`, `[layout]` + panes, `[[timeline]]` actions).
- **Export targets**: `cast` (asciinema v2) and `html` (self-contained player) —
  pure Rust; `gif` — pure Rust rasterizer (vt100 + embedded DejaVu Sans Mono);
  `mp4` — H.264 via ffmpeg.
- **Multi-scene stage**: terminal + `browser` panes composited onto a shared
  canvas (the Stage Matrix), driving headless Chromium for browser panes.
- **Tectonic-style provisioning**: `mp4` auto-fetches a managed ffmpeg and browser
  panes auto-fetch Chromium on first use; a system install is preferred if present.
- A clean `PS1='$ '` is forced during export so demos never leak `user@host`.

### Notes
- `cast`/`html`/`gif` and the core pipeline are fully offline. The Chromium screenshot
  path is exercised on machines with Chromium available.
