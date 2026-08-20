#!/bin/sh
# PlexusPact GitHub Action runner. No business logic lives here: download a
# pinned binary, verify its checksum, run `plexuspact check`, translate the
# JSON result into workflow annotations, and propagate the exit code.
set -eu

VERSION="${INPUT_VERSION:?version input is required}"
REPO="${GH_REPO:?}"
BASE_URL="https://github.com/${REPO}/releases/download/v${VERSION}"

# --- 1. Resolve the artifact for this runner ---------------------------------
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

# --- 2. Download and verify --------------------------------------------------
tool_dir="${RUNNER_TEMP:-/tmp}/plexuspact-${VERSION}"
mkdir -p "$tool_dir"
echo "Downloading ${BASE_URL}/${archive}"
curl -fsSL --retry 3 -o "${tool_dir}/${archive}" "${BASE_URL}/${archive}"
curl -fsSL --retry 3 -o "${tool_dir}/SHA256SUMS" "${BASE_URL}/SHA256SUMS"

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

# --- 3. Run the check --------------------------------------------------------
format="${INPUT_FORMAT:-json}"
case "$format" in
  json)  results_file="plexuspact-results.json" ;;
  junit) results_file="plexuspact-results.xml" ;;
  human) results_file="plexuspact-results.txt" ;;
  *) echo "::error::invalid format input: ${format} (expected json, junit, or human)"; exit 2 ;;
esac

set -- check "$INPUT_DATA" --contract "$INPUT_CONTRACT" --format "$format" --report plexuspact-report.html
[ "${INPUT_STRICT:-false}" = "true" ] && set -- "$@" --strict

exit_code=0
"$bin" "$@" > "$results_file" || exit_code=$?
echo "exit-code=${exit_code}" >> "$GITHUB_OUTPUT"
echo "results-file=${results_file}" >> "$GITHUB_OUTPUT"

# --- 4. Annotate the PR from the JSON result ---------------------------------
if [ "$format" = "json" ] && [ -s "$results_file" ] && jq -e . "$results_file" >/dev/null 2>&1; then
  jq -r --arg file "$INPUT_CONTRACT" '
    .checks[]
    | select(.status == "failed")
    | (if .severity == "warn" then "warning" else "error" end) as $level
    # Row metrics are absent for dataset-level checks (freshness, row_count) —
    # fall back to the check message so annotations never read "null of null".
    | (if .metrics.rows_failed != null
         then "\(.metrics.rows_failed) rows failed of \(.metrics.rows_evaluated)"
         else (.message // "check failed") end) as $detail
    | "::\($level) file=\($file),title=\(.id)::\(.column // .kind) failed \(.kind) (\($detail))"
  ' "$results_file"
  jq -r '"plexuspact: \(.summary.passed)/\(.summary.checks_total) checks passed on \(.source.rows) rows (\(.status))"' "$results_file"
elif [ "$format" = "json" ]; then
  echo "::warning::plexuspact produced no parseable JSON result (exit ${exit_code}); see the job log"
else
  cat "$results_file"
  echo "::notice::PR annotations are only emitted with format: json"
fi

exit "$exit_code"
