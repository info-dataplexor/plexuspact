#!/bin/sh
# PlexusPact contract preflight. Asks the registry what this branch's contract
# would do, comments the answer on the pull request, and fails the job when the
# change tightens terms somebody relies on.
#
# No business logic lives here either: the CLI renders both the human output and
# the Markdown, so the comment on a pull request and the text in a terminal
# cannot describe the change differently.
set -eu

# --- 1. Download and verify the pinned binary --------------------------------
VERSION="${INPUT_VERSION:?version input is required}"
REPO="${GH_REPO:?}"
. "$(dirname "$0")/../install.sh"

# --- 2. Ask the registry -----------------------------------------------------
summary_file="plexuspact-preflight.md"
json_file="plexuspact-preflight.json"

set -- diff "$INPUT_CONTRACT" --against-registry --json --markdown "$summary_file"
[ -n "${INPUT_DATASET:-}" ] && set -- "$@" --dataset "$INPUT_DATASET"

exit_code=0
"$bin" "$@" > "$json_file" || exit_code=$?

# Exit 3 is the registry never answering. That is not a passing build: the
# question was asked and nothing came back, and a job that calls that "no
# breaking changes" stops catching them the first time the network is slow.
if [ "$exit_code" = "3" ]; then
  echo "::error::plexuspact could not reach the registry; nothing was compared"
  exit 3
fi

verdict="$(jq -r '.verdict // ""' "$json_file" 2>/dev/null || echo "")"
breaking="$(jq -r '.breaking // 0' "$json_file" 2>/dev/null || echo 0)"
requires_review="$(jq -r '.requires_review // false' "$json_file" 2>/dev/null || echo false)"
summary="$(jq -r '.summary // ""' "$json_file" 2>/dev/null || echo "")"
{
  echo "verdict=${verdict}"
  echo "breaking=${breaking}"
  echo "requires-review=${requires_review}"
  echo "summary-file=${summary_file}"
} >> "$GITHUB_OUTPUT"

# --- 3. Annotate the contract file -------------------------------------------
# One annotation per change, on the file that changed, so the reviewer reads it
# where they are already looking rather than in a job log.
if [ -s "$json_file" ] && jq -e . "$json_file" >/dev/null 2>&1; then
  jq -r --arg file "$INPUT_CONTRACT" '
    .changes[]?
    | select(.impact == "breaking")
    | "::error file=\($file),title=Breaking change::\(.path) — \(.description)"
  ' "$json_file"
  jq -r --arg file "$INPUT_CONTRACT" '
    .impact.consumers[]?
    | "::warning file=\($file),title=Downstream::\(.name) reads what this change touches"
  ' "$json_file"
fi

if [ "$requires_review" = "true" ]; then
  echo "::notice::Merging will record this as proposed; the version already in force keeps applying until somebody approves it"
fi

# --- 4. The job summary, always ----------------------------------------------
[ -s "$summary_file" ] && cat "$summary_file" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"

# --- 5. The pull-request comment ---------------------------------------------
# Updated in place rather than appended, because a contract that goes through
# four pushes should leave one comment saying what is true now, not four saying
# what used to be.
marker="<!-- plexuspact-preflight -->"
pr="$(jq -r '.pull_request.number // .number // empty' "${GITHUB_EVENT_PATH:-/dev/null}" 2>/dev/null || echo "")"
if [ "${INPUT_COMMENT:-true}" = "true" ] && [ -n "$pr" ] && [ -s "$summary_file" ]; then
  body_file="${RUNNER_TEMP:-/tmp}/plexuspact-comment.json"
  printf '%s\n\n' "$marker" | cat - "$summary_file" > "${body_file}.md"
  jq -Rs '{body: .}' < "${body_file}.md" > "$body_file"

  existing="$(gh api "repos/${GITHUB_REPOSITORY}/issues/${pr}/comments" --paginate \
    --jq "map(select(.body | startswith(\"${marker}\"))) | .[0].id // empty" 2>/dev/null || echo "")"
  if [ -n "$existing" ]; then
    gh api -X PATCH "repos/${GITHUB_REPOSITORY}/issues/comments/${existing}" --input "$body_file" >/dev/null \
      || echo "::warning::could not update the preflight comment (needs pull-requests: write)"
  else
    gh api -X POST "repos/${GITHUB_REPOSITORY}/issues/${pr}/comments" --input "$body_file" >/dev/null \
      || echo "::warning::could not post the preflight comment (needs pull-requests: write)"
  fi
fi

# --- 6. The verdict ----------------------------------------------------------
case "$verdict" in
  invalid)
    echo "::error::${summary}"
    exit 2 ;;
  breaking)
    if [ "${INPUT_FAIL_ON_BREAKING:-true}" = "true" ]; then
      echo "::error::${summary}"
      exit 1
    fi
    echo "::warning::${summary}" ;;
  *)
    echo "::notice::${summary}" ;;
esac

exit 0
