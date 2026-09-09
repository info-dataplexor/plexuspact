# Installation

PlexusPact ships as a single static binary — no Python, no JVM, no shared libraries. Prebuilt binaries cover x86_64 Linux (musl, fully static), macOS (Intel and Apple Silicon), and x86_64 Windows.

## Shell script (Linux, macOS)

```sh
curl -fsSL https://raw.githubusercontent.com/info-dataplexor/plexuspact/main/install.sh | sh
```

The script detects your OS/architecture, downloads the release from GitHub Releases, **verifies the SHA256 checksum**, and installs to `~/.local/bin` (or `/usr/local/bin` as a fallback). Pin a version or change the destination with environment variables:

```sh
VERSION=0.2.0 INSTALL_DIR=/opt/bin sh -c "$(curl -fsSL https://raw.githubusercontent.com/info-dataplexor/plexuspact/main/install.sh)"
```

## PowerShell (Windows)

```powershell
irm https://raw.githubusercontent.com/info-dataplexor/plexuspact/main/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\Programs\plexuspact` and adds it to your user `PATH`. Pin with `$env:VERSION = "0.2.0"` before running.

## Homebrew (macOS, Linux)

```sh
brew install plexuspact-io/tap/plexuspact
```

## Scoop (Windows)

```powershell
scoop bucket add plexuspact-io https://github.com/plexuspact-io/scoop-bucket
scoop install plexuspact
```

## Cargo (build from source)

Requires a recent stable Rust toolchain (MSRV 1.88):

```sh
cargo install --git https://github.com/info-dataplexor/plexuspact plexuspact-cli
```

## Manual download

Grab an archive and `SHA256SUMS` from the [releases page](https://github.com/info-dataplexor/plexuspact/releases), verify, and put the binary on your `PATH`:

```sh
sha256sum -c --ignore-missing SHA256SUMS
tar -xzf plexuspact-v0.2.0-x86_64-unknown-linux-musl.tar.gz
sudo install -m 755 plexuspact /usr/local/bin/
```

## Verify

```sh
plexuspact --version
```

You should see `plexuspact 0.2.0` (or newer). Now head to the [quickstart](quickstart.md).

## CI

Don't hand-install in CI — use the [official GitHub Action](ci-recipes.md#github-actions), which downloads a pinned, checksum-verified binary, or copy the pattern from the [other CI recipes](ci-recipes.md).
