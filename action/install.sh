#!/bin/sh
# Download a pinned plexuspact release and verify it, leaving the executable's
# path in $bin.
#
# Sourced rather than run, and shared by both actions in this directory. There
# is exactly one place that decides which artifact a runner gets and exactly one
# place that checks a checksum, because a second copy of this drifting is how a
# release ends up verified on Linux and not on macOS without anybody noticing.
#
# Expects: VERSION, REPO. Sets: bin.

VERSION="${VERSION:?version is required}"
REPO="${REPO:?}"
BASE_URL="https://github.com/${REPO}/releases/download/v${VERSION}"

case "$(uname -s)" in
  Linux)  target="x86_64-unknown-linux-musl"; ext="tar.gz" ;;
  Darwin)
    case "$(uname -m)" in
      arm64|aarch64) target="aarch64-apple-darwin" ;;
      *)             target="x86_64-apple-darwin" ;;
    esac
    ext="tar.gz" ;;
  MINGW*|MSYS*|CYGWIN*|Windows_NT) target="x86_64-pc-windows-msvc"; ext="zip" ;;
  *) echo "::error::unsupported runner OS: $(uname -s)"; exit 3 ;;
esac
archive="plexuspact-v${VERSION}-${target}.${ext}"

tool_dir="${RUNNER_TEMP:-/tmp}/plexuspact-${VERSION}"
mkdir -p "$tool_dir"
echo "Downloading ${BASE_URL}/${archive}"
for file in "$archive" SHA256SUMS; do
  if ! curl -fsSL --retry 3 -o "${tool_dir}/${file}" "${BASE_URL}/${file}"; then
    echo "::error::could not download ${BASE_URL}/${file} - is v${VERSION} a published release of ${REPO}?"
    exit 3
  fi
done

if command -v sha256sum >/dev/null 2>&1; then sum_cmd="sha256sum"; else sum_cmd="shasum -a 256"; fi
expected="$(grep " ${archive}\$" "${tool_dir}/SHA256SUMS" | awk '{print $1}')"
actual="$(cd "$tool_dir" && $sum_cmd "$archive" | awk '{print $1}')"
if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
  echo "::error::checksum verification failed for ${archive} (expected ${expected:-<missing>}, got ${actual})"
  exit 3
fi

case "$ext" in
  tar.gz) tar -xzf "${tool_dir}/${archive}" -C "$tool_dir" ;;
  zip)    powershell.exe -NoProfile -Command \
            "Expand-Archive -Force -LiteralPath '${tool_dir}/${archive}' -DestinationPath '${tool_dir}'" ;;
esac
bin="${tool_dir}/plexuspact"
[ -f "${bin}.exe" ] && bin="${bin}.exe"
chmod +x "$bin"
"$bin" --version
