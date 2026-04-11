#!/usr/bin/env bash
set -euo pipefail

INPUT="$(cat)"

if ! command -v jq >/dev/null 2>&1; then
  exit 0
fi

if [[ "$(jq -r '.stop_hook_active // false' <<<"$INPUT")" == "true" ]]; then
  exit 0
fi

CWD="$(jq -r '.cwd // empty' <<<"$INPUT")"
if [[ -n "$CWD" ]]; then
  cd "$CWD"
fi

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  exit 0
fi

if git diff --quiet && git diff --cached --quiet && [[ -z "$(git ls-files --others --exclude-standard)" ]]; then
  exit 0
fi

jq -n --arg reason 'Invoke the bundled Claude Code skill exactly as: /loop request a review of the changes from Codex and repeat the modifications until there are no further comments' \
  '{
    "decision": "block",
    "reason": $reason
  }'
