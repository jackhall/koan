#!/usr/bin/env bash
# UserPromptSubmit hook. Fires once per turn (not per Edit/Write). Emits the
# `documentation` skill reminder only when this turn looks doc-shaped:
#   (a) the user prompt mentions docs/roadmap/design/README/tutorial/CHANGELOG, OR
#   (b) the working tree already has pending edits to the koan doc tree.
# Non-blocking: always exits 0.

set -u

repo=/var/home/jack/Code/koan

prompt=$(python3 -c 'import sys, json; d=json.load(sys.stdin); print(d.get("prompt",""))' 2>/dev/null)

prompt_hit=0
if printf '%s' "$prompt" | grep -qE -i '\b(doc|docs|documentation|roadmap|design|readme|tutorial|changelog|doclinks)\b'; then
  prompt_hit=1
fi

git_hit=0
if [ "$prompt_hit" -eq 0 ]; then
  if git -C "$repo" status --porcelain 2>/dev/null | \
       awk '{print $NF}' | \
       grep -qE '^(README\.md|tutorial/|ROADMAP\.md|design/|roadmap/)'; then
    git_hit=1
  fi
fi

if [ "$prompt_hit" -eq 1 ] || [ "$git_hit" -eq 1 ]; then
  cat <<'JSON'
{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"This turn looks doc-shaped. If you have not already invoked the `documentation` skill this session, do so before editing the koan doc tree (README.md, tutorial/, ROADMAP.md, design/, roadmap/) — it carries the partition rules and the doclinks workflow (problem-vs-impact partition, design-doc no-historical-narrative rule, and the doclinks check/deps/orphans gating triple)."}}
JSON
fi

exit 0
