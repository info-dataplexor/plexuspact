#!/usr/bin/env sh
# pre-commit hook: refuse a commit that changes a contract in a way its
# consumers would feel. Each staged contract is compared with the version in
# HEAD by `plexuspact diff`, which exits 1 on a breaking change (a column or
# check removed, a type changed, a column made required, the key changed…).
# A contract that is new in this commit has nothing to break and is skipped;
# so is every file that is not a contract.
set -u
. "$(dirname "$0")/common.sh"

old="$(mktemp "${TMPDIR:-/tmp}/plexuspact-head.XXXXXX")"
trap 'rm -f "$old"' EXIT

status=0
for file in "$@"; do
  is_contract "$file" || continue
  git cat-file -e "HEAD:${file}" 2>/dev/null || continue
  git show "HEAD:${file}" > "$old"
  if ! "$bin" diff "$old" "$file"; then
    echo "plexuspact diff: ${file} breaks consumers of the version in HEAD" >&2
    status=1
  fi
done
exit $status
