#!/usr/bin/env sh
# pre-commit hook: parse and lint every staged PlexusPact contract (or ODCS
# document) with `plexuspact validate-contract`. Files that are not contracts
# are skipped, so `types: [yaml]` is a safe filter.
set -u
. "$(dirname "$0")/common.sh"

status=0
for file in "$@"; do
  is_contract "$file" || continue
  "$bin" validate-contract "$file" || status=1
done
exit $status
