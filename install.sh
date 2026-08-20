#!/bin/sh
# PlexusPact installer for Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/dataplexor/plexuspact/main/install.sh | sh
#
# Environment variables:
#   VERSION      install a specific version (e.g. VERSION=0.1.0); default: latest release
#   INSTALL_DIR  install directory; default: ~/.local/bin, falling back to /usr/local/bin
set -eu

REPO="dataplexor/plexuspact"
BIN="plexuspact"

say()  { printf '%s\n' "install.sh: $*"; }
fail() { printf '%s\n' "install.sh: error: $*" >&2; exit 1; }

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar  >/dev/null 2>&1 || fail "tar is required"

# --- Detect platform ----------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)
    case "$arch" in
      x86_64|amd64) target="x86_64-unknown-linux-musl" ;;
      *) fail "unsupported Linux architecture: $arch (prebuilt binaries cover x86_64; use 'cargo install --git https://github.com/${REPO} plexuspact-cli')" ;;
    esac ;;
  Darwin)
    case "$arch" in
      arm64|aarch64) target="aarch64-apple-darwin" ;;
      x86_64)        target="x86_64-apple-darwin" ;;
      *) fail "unsupported macOS architecture: $arch" ;;
    esac ;;
  *)
    fail "unsupported OS: $os (on Windows, use install.ps1 or Scoop)" ;;
esac

# --- Resolve version -----------------------------------------------------------
if [ -n "${VERSION:-}" ]; then
  version="${VERSION#v}"
else
  say "resolving latest release..."
  latest_url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/${REPO}/releases/latest")" \
    || fail "could not reach github.com to resolve the latest release"
  version="${latest_url##*/v}"
  [ -n "$version" ] && [ "$version" != "$latest_url" ] || fail "could not parse latest version from ${latest_url}"
fi
say "installing ${BIN} v${version} (${target})"

# --- Download and verify -------------------------------------------------------
archive="${BIN}-v${version}-${target}.tar.gz"
base_url="https://github.com/${REPO}/releases/download/v${version}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL --retry 3 -o "${tmp}/${archive}" "${base_url}/${archive}" \
  || fail "download failed: ${base_url}/${archive}"
curl -fsSL --retry 3 -o "${tmp}/SHA256SUMS" "${base_url}/SHA256SUMS" \
  || fail "download failed: ${base_url}/SHA256SUMS"

if command -v sha256sum >/dev/null 2>&1; then sum_cmd="sha256sum"; else sum_cmd="shasum -a 256"; fi
expected="$(grep " ${archive}\$" "${tmp}/SHA256SUMS" | awk '{print $1}')"
[ -n "$expected" ] || fail "no checksum for ${archive} in SHA256SUMS"
actual="$(cd "$tmp" && $sum_cmd "$archive" | awk '{print $1}')"
[ "$expected" = "$actual" ] || fail "checksum mismatch for ${archive}: expected ${expected}, got ${actual}"
say "checksum verified"

tar -xzf "${tmp}/${archive}" -C "$tmp"
[ -f "${tmp}/${BIN}" ] || fail "archive did not contain the ${BIN} binary"

# --- Install -------------------------------------------------------------------
if [ -n "${INSTALL_DIR:-}" ]; then
  dir="$INSTALL_DIR"
elif [ -d "${HOME}/.local/bin" ] || mkdir -p "${HOME}/.local/bin" 2>/dev/null; then
  dir="${HOME}/.local/bin"
else
  dir="/usr/local/bin"
fi

if [ -w "$dir" ]; then
  install -m 755 "${tmp}/${BIN}" "${dir}/${BIN}"
else
  say "${dir} is not writable; retrying with sudo"
  sudo install -m 755 "${tmp}/${BIN}" "${dir}/${BIN}"
fi

say "installed ${dir}/${BIN}"
if ! command -v "$BIN" >/dev/null 2>&1; then
  say "note: ${dir} is not on your PATH. Add it, e.g.:"
  say "  export PATH=\"${dir}:\$PATH\""
fi
say "run '${BIN} --version' to verify, then '${BIN} init your_data.csv' to get started."
