#!/usr/bin/env bash
# Deterministic GitHub API fixtures: no network, token or repository writes.
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=scripts/ci/coderabbit-gate-lib.sh
. "$script_dir/coderabbit-gate-lib.sh"

test_dir=$(mktemp -d)
trap 'rm -rf -- "$test_dir"' EXIT
calls=$test_dir/calls
GH_REPO=owner/project
LABEL=ci-green
HEAD_SHA=current-head
PR=384
# Fork event metadata is untrusted and never becomes the write repository.
GITHUB_HEAD_REPOSITORY=attacker/fork
export GH_REPO LABEL HEAD_SHA PR GITHUB_HEAD_REPOSITORY
mock_live_head=$HEAD_SHA
delete_status=0
mock_not_green=
mock_pulls='[
  {"number":1,"state":"open","head":{"sha":"old-head"},"base":{"repo":{"full_name":"owner/project"}}},
  {"number":2,"state":"open","head":{"sha":"current-head"},"base":{"repo":{"full_name":"attacker/fork"}}},
  {"number":3,"state":"closed","head":{"sha":"current-head"},"base":{"repo":{"full_name":"owner/project"}}},
  {"number":384,"state":"open","head":{"sha":"current-head"},"base":{"repo":{"full_name":"owner/project"}}}
]'

gh() {
  printf '%s\n' "$*" >>"$calls"
  case "$*" in
    "api repos/$GH_REPO/pulls/$PR "*) printf '%s\n' "$mock_live_head" ;;
    "api -X DELETE repos/$GH_REPO/issues/$PR/labels/$LABEL --silent")
      if [ "$delete_status" -ne 0 ]; then
        printf 'gh: forbidden or absent (HTTP %s)\n' "$delete_status" >&2
        return 1
      fi
      ;;
    "api repos/$GH_REPO/commits/$HEAD_SHA/pulls") printf '%s\n' "$mock_pulls" ;;
    "api repos/$GH_REPO/commits/$HEAD_SHA/check-runs?per_page=100 "*)
      printf '%s\n' "$mock_not_green"
      ;;
    "label create $LABEL "*) ;;
    "pr edit $PR --add-label $LABEL") ;;
    *) printf 'Unexpected mock gh call: %s\n' "$*" >&2; return 1 ;;
  esac
}

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
reset_calls() { : >"$calls"; }
assert_no_write() {
  if grep -Eq 'DELETE|^label |^pr edit ' "$calls"; then
    fail "unexpected label write: $(cat "$calls")"
  fi
}

reset_calls
park_gate "$PR" "$HEAD_SHA" >"$test_dir/out"
grep -Fq "DELETE repos/owner/project/issues/384/labels/ci-green" "$calls" \
  || fail "fork park did not target the owning repository PR"
if grep -Fq "$GITHUB_HEAD_REPOSITORY" "$calls"; then
  fail "fork repository metadata reached the API target"
fi
echo 'PASS: fork park writes only the owning repository'

reset_calls
mock_live_head=newer-head
park_gate "$PR" "$HEAD_SHA" >"$test_dir/out"
assert_no_write
echo 'PASS: stale park skips writes'

reset_calls
mock_live_head=$HEAD_SHA
delete_status=404
park_gate "$PR" "$HEAD_SHA" >"$test_dir/out" 2>"$test_dir/err"
[ ! -s "$test_dir/err" ] || fail "missing label produced an error"
echo 'PASS: HTTP 404 is quiet'

reset_calls
delete_status=403
if park_gate "$PR" "$HEAD_SHA" >"$test_dir/out" 2>"$test_dir/err"; then
  fail "HTTP 403 was swallowed"
fi
grep -Fq 'HTTP 403' "$test_dir/err" || fail "HTTP 403 diagnostic was hidden"
echo 'PASS: HTTP 403 fails loudly'
delete_status=0

reset_calls
pulls_before=$mock_pulls
mock_pulls='[{"number":384,"state":"open","head":{"sha":"old-head"},"base":{"repo":{"full_name":"owner/project"}}}]'
grant_gate_when_green "$HEAD_SHA" >"$test_dir/out"
assert_no_write
echo 'PASS: completed checks cannot select an older PR head'
mock_pulls=$pulls_before

reset_calls
mock_live_head=newer-head
grant_gate_when_green "$HEAD_SHA" >"$test_dir/out"
assert_no_write
echo 'PASS: head is revalidated before granting'

reset_calls
mock_live_head=$HEAD_SHA
grant_gate_when_green "$HEAD_SHA" >"$test_dir/out"
grep -Fxq "pr edit 384 --add-label ci-green" "$calls" \
  || fail "current own PR was not granted the label"
echo 'PASS: green current head grants the owning PR'

reset_calls
mock_not_green=contracts
grant_gate_when_green "$HEAD_SHA" >"$test_dir/out"
grep -Fq "DELETE repos/owner/project/issues/384/labels/ci-green" "$calls" \
  || fail "failed checks did not withdraw the label"
if grep -q '^pr edit ' "$calls"; then
  fail "failed checks granted a label"
fi
echo 'PASS: failed current checks withdraw the label'
