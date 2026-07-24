#!/usr/bin/env bash
# UserPromptSubmit hook. Fires at most once per session: emits the
# `documentation` skill reminder the first time the working tree has
# pending edits to the koan doc tree. Non-blocking: always exits 0.

set -u

repo=/var/home/jack/Code/koan

session_id=$(python3 -c 'import sys, json; d=json.load(sys.stdin); print(d.get("session_id",""))' 2>/dev/null)
[ -n "$session_id" ] || exit 0

sentinel="/tmp/claude-doc-skill-reminder-fired-${session_id}"
[ -e "$sentinel" ] && exit 0

if git -C "$repo" status --porcelain 2>/dev/null | \
     awk '{print $NF}' | \
     grep -qE '^(README\.md|tutorial/|ROADMAP\.md|design/|roadmap/)'; then
  touch "$sentinel"
  cat <<'JSON'
{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"This turn looks doc-shaped. If you have not already invoked the `documentation` skill this session, do so before editing the koan doc tree (README.md, tutorial/, ROADMAP.md, design/, roadmap/)."}}
JSON
fi

exit 0
