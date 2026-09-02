#!/bin/sh
# Put a merged contract into force, and say where it was agreed.
#
# The whole reason this exists rather than a `curl` in somebody's workflow is
# the last part. A contract pushed by an API key arrives from nowhere: the
# record says a machine did it and stops. The pull request is the real answer to
# "who agreed to this" — it has the reviewers, the approvals and the argument
# attached — and the merge job is the only place that knows its URL.
set -eu

# --- 1. Download and verify the pinned binary --------------------------------
VERSION="${INPUT_VERSION:?version input is required}"
REPO="${GH_REPO:?}"
. "$(dirname "$0")/../install.sh"

# --- 2. Work out where this version was agreed -------------------------------
# Precedence: what the caller said, then the pull request this commit came from,
# then the commit itself. Never a guess — if none of these are known the field
# is left empty, because a provenance link that goes somewhere unrelated is
# worse than an honest blank.
source_url="${INPUT_SOURCE_URL:-}"
if [ -z "$source_url" ]; then
  # On `pull_request` events the number is in the event payload. On a `push` to
  # the default branch it is not, so ask the API which pull request contained
  # this commit — that is the merge, which is the agreement.
  pr_url="$(jq -r '.pull_request.html_url // empty' "${GITHUB_EVENT_PATH:-/dev/null}" 2>/dev/null || echo "")"
  if [ -z "$pr_url" ] && [ -n "${GITHUB_SHA:-}" ]; then
    pr_url="$(gh api "repos/${GITHUB_REPOSITORY}/commits/${GITHUB_SHA}/pulls" \
      --jq '.[0].html_url // empty' 2>/dev/null || echo "")"
  fi
  if [ -n "$pr_url" ]; then
    source_url="$pr_url"
  elif [ -n "${GITHUB_SHA:-}" ]; then
    source_url="${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY}/commit/${GITHUB_SHA}"
  fi
fi

version_label="${INPUT_VERSION_LABEL:-}"
if [ -z "$version_label" ] && [ -n "${GITHUB_SHA:-}" ]; then
  version_label="$(printf '%s' "$GITHUB_SHA" | cut -c1-7)"
fi

# --- 3. Register -------------------------------------------------------------
set -- register "$INPUT_CONTRACT"
[ -n "${INPUT_DATASET:-}" ] && set -- "$@" --dataset "$INPUT_DATASET"
[ -n "$version_label" ] && set -- "$@" --version-label "$version_label"
[ -n "$source_url" ] && set -- "$@" --source-url "$source_url"
[ "${INPUT_FAIL_IF_PENDING:-false}" = "true" ] && set -- "$@" --fail-if-pending

exit_code=0
"$bin" "$@" | tee plexuspact-register.txt || exit_code=$?

status="active"
if grep -q "recorded as proposed" plexuspact-register.txt 2>/dev/null; then
  status="proposed"
  echo "::notice::the contract was recorded and is waiting for an approver; the version already in force keeps applying"
fi
{
  echo "status=${status}"
  echo "source-url=${source_url}"
} >> "$GITHUB_OUTPUT"

exit "$exit_code"
