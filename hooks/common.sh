#!/usr/bin/env sh
# Shared by the pre-commit hooks in this directory: decides which files a hook
# looks at and puts a verified `plexuspact` executable in $bin.
#
# Sourced, not run. Sets: bin. Defines: is_contract.
#
# The executable is the release matching this checkout's version — `rev:` in
# .pre-commit-config.yaml picks the tag, the tag's Cargo.toml names the
# version — downloaded once into the user's cache through the same
# checksum-verified installer the GitHub Action uses. Two overrides:
#   PLEXUSPACT_BIN      use this executable instead (a local build, a package
#                       manager's copy, an air-gapped mirror); nothing is fetched
#   PLEXUSPACT_VERSION  fetch this release instead of the checkout's own

HOOK_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# A PlexusPact contract starts with `apiVersion:` and names a `dataset:`; an
# ODCS document says `kind: DataContract`. Anything else — Kubernetes
# manifests, CI config, dbt schemas — is left alone without a word, so the
# hooks can run over every YAML file in a repository.
is_contract() {
  grep -qE '^kind:[[:space:]]*DataContract[[:space:]]*$' "$1" 2>/dev/null && return 0
  grep -qE '^apiVersion:' "$1" 2>/dev/null && grep -qE '^dataset:' "$1" 2>/dev/null
}

if [ -n "${PLEXUSPACT_BIN:-}" ]; then
  bin="$PLEXUSPACT_BIN"
  if [ ! -x "$bin" ] && ! command -v "$bin" >/dev/null 2>&1; then
    echo "plexuspact: PLEXUSPACT_BIN=${bin} is not an executable" >&2
    exit 3
  fi
else
  VERSION="${PLEXUSPACT_VERSION:-$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$HOOK_ROOT/Cargo.toml" | head -n 1)}"
  if [ -z "$VERSION" ]; then
    echo "plexuspact: could not read the version from ${HOOK_ROOT}/Cargo.toml" >&2
    exit 3
  fi
  cache="${PLEXUSPACT_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/plexuspact}"
  bin="${cache}/plexuspact-${VERSION}/plexuspact"
  [ -f "${bin}.exe" ] && bin="${bin}.exe"
  if [ ! -x "$bin" ]; then
    mkdir -p "$cache"
    echo "plexuspact: fetching v${VERSION} into ${cache} (set PLEXUSPACT_BIN to use a local executable)" >&2
    REPO="info-dataplexor/plexuspact"
    RUNNER_TEMP="$cache"
    . "$HOOK_ROOT/action/install.sh"
  fi
fi
