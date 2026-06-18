# Contributing to demo-stage

Thank you for your interest in contributing!

## Development workflow

```bash
git clone https://github.com/UniverLab/demo-stage.git
cd demo-stage
cargo build
cargo test
```

Branch off `develop` for all changes. PRs go to `develop`, not `main`.

## CI/CD and required secrets

This project uses GitHub Actions for CI (`.github/workflows/ci.yml`) and automated releases (`.github/workflows/release.yml`).

### Required repository secrets

| Secret | Description | Where to get it |
|---|---|---|
| `CARGO_REGISTRY_TOKEN` | API token to publish crates to [crates.io](https://crates.io) | [crates.io/me](https://crates.io/me) → API Tokens → New Token |

Configure secrets at:  
`https://github.com/UniverLab/demo-stage/settings/secrets/actions`

> **Tip:** If you use [ghscaff](https://github.com/UniverLab/ghscaff), you can run `ghscaff apply` to configure missing secrets interactively, or set the env var before running:
> ```bash
> export CARGO_REGISTRY_TOKEN=<your_token>
> ghscaff apply
> ```

## Release process

Releases are automated via the `release.yml` workflow. To cut a release:

1. Bump the version in `Cargo.toml`
2. Commit: `chore: release vX.Y.Z`
3. Tag: `git tag vX.Y.Z && git push origin vX.Y.Z`

The workflow builds binaries for Linux, macOS, and Windows, publishes to crates.io, and creates a GitHub Release.

## Code style

```bash
cargo fmt        # format
cargo clippy -- -D warnings   # lint
cargo test       # test
```

All three must pass before merging.
