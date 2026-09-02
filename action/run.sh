#!/bin/sh
# PlexusPact GitHub Action runner. No business logic lives here: download a
# pinned binary, verify its checksum, run `plexuspact check`, translate the
# JSON result into workflow annotations, and propagate the exit code.
set -eu

# --- 1+2. Download and verify the pinned binary ------------------------------
VERSION="${INPUT_VERSION:?version input is required}"
REPO="${GH_REPO:?}"
. "$(dirname "$0")/install.sh"

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
[ "${INPUT_REDACT_SAMPLES:-false}" = "true" ] && set -- "$@" --redact-samples
# The CLI reports to the cloud on its own whenever PLEXUSPACT_API_KEY is set;
# --push only changes a failure to report from a warning into an error.
[ -n "${PLEXUSPACT_API_KEY:-}" ] && [ "${INPUT_REQUIRE_PUSH:-false}" = "true" ] && set -- "$@" --push

# stderr is captured rather than inherited so the run URL can be turned into an
# annotation, then replayed verbatim so nothing is swallowed.
err_file="${RUNNER_TEMP:-/tmp}/plexuspact-stderr.log"
exit_code=0
"$bin" "$@" > "$results_file" 2> "$err_file" || exit_code=$?
cat "$err_file" >&2
echo "exit-code=${exit_code}" >> "$GITHUB_OUTPUT"
echo "results-file=${results_file}" >> "$GITHUB_OUTPUT"

# --- 3b. Surface where the run landed ----------------------------------------
if [ -n "${PLEXUSPACT_API_KEY:-}" ]; then
  run_url="$(sed -n 's/.*reported run to //p' "$err_file" | tail -1)"
  if [ -n "$run_url" ]; then
    echo "run-url=${run_url}" >> "$GITHUB_OUTPUT"
    echo "::notice::Run recorded at ${run_url}"
  else
    echo "::warning::plexuspact could not record this run; see the log above"
  fi
fi

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
