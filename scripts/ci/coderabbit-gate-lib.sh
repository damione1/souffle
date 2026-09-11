#!/usr/bin/env bash
# Helpers shared by the two jobs of .github/workflows/coderabbit-gate.yml.
# Sourced, not executed: it defines functions and reads GH_REPO and LABEL from
# the workflow environment.

# Remove the gate label from a pull request.
#
# A label that was not set is the ordinary case and must stay quiet, but a token
# that is not allowed to write labels has to be loud: swallowing both left a 403
# looking exactly like a 404, and the gate would have reported itself healthy
# while never releasing a single review.
drop_label() {
  local pr=$1 err
  if err=$(gh api -X DELETE "repos/$GH_REPO/issues/$pr/labels/$LABEL" --silent 2>&1); then
    echo "gate withdrawn from #$pr"
  elif printf '%s' "$err" | grep -q 'HTTP 404'; then
    echo "gate was not set on #$pr"
  else
    printf 'cannot remove %s from #%s:\n%s\n' "$LABEL" "$pr" "$err" >&2
    return 1
  fi
}
