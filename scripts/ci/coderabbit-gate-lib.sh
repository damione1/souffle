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

# Event SHAs identify the proposed decision; the live PR decides whether it
# still applies. All API writes target GH_REPO, including for fork heads.
current_open_head() {
  gh api "repos/$GH_REPO/pulls/$1" \
    --jq 'if .state == "open" then .head.sha else empty end'
}

park_gate() {
  local pr=$1 head=$2 live_head
  live_head=$(current_open_head "$pr") || return
  if [ "$live_head" != "$head" ]; then
    echo "Skipping stale park for #$pr at $head"
    return 0
  fi
  drop_label "$pr"
}

grant_gate_when_green() {
  local head=$1 pulls pr not_green live_head
  pulls=$(gh api "repos/$GH_REPO/commits/$head/pulls") || return
  pr=$(jq -r --arg head "$head" --arg repo "$GH_REPO" \
    '[.[] | select(.state == "open" and .head.sha == $head
                   and .base.repo.full_name == $repo)] | first | .number // empty' \
    <<<"$pulls") || return
  if [ -z "$pr" ]; then
    echo "No open PR with current head $head"
    return 0
  fi

  # Ignore CodeRabbit and this workflow itself: neither may gate its own run.
  not_green=$(gh api "repos/$GH_REPO/commits/$head/check-runs?per_page=100" \
    --jq '[.check_runs[]
           | select(.app.slug == "github-actions")
           | select(.name | startswith("coderabbit-") | not)
           | select((.status == "completed"
                     and (.conclusion == "success"
                          or .conclusion == "skipped"
                          or .conclusion == "neutral")) | not)
           | .name] | unique | join(", ")') || return

  live_head=$(current_open_head "$pr") || return
  if [ "$live_head" != "$head" ]; then
    echo "Skipping stale CI result for #$pr at $head"
    return 0
  fi
  if [ -n "$not_green" ]; then
    echo "Still pending or failed: $not_green"
    drop_label "$pr"
    return
  fi

  # GitHub's head read and label write are separate requests. If a push lands
  # after this read, its serialized park event withdraws the label afterward.
  gh label create "$LABEL" --color 0e8a16 \
    --description "CI green: CodeRabbit may review" 2>/dev/null || true
  gh pr edit "$pr" --add-label "$LABEL" || return
  echo "PR #$pr is green, CodeRabbit released"
}
