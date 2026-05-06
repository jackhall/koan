#!/usr/bin/env bash
# PreToolUse hook fired on Edit and Write. When the target path is in the
# koan doc tree (README.md, TUTORIAL.md, ROADMAP.md, design/*.md, roadmap/*.md),
# emits an additionalContext nudge reminding Claude to invoke the
# `documentation` skill. Non-blocking: always exits 0.

file_path=$(python3 -c 'import sys, json; d=json.load(sys.stdin); print(d.get("tool_input",{}).get("file_path",""))' 2>/dev/null)

case "$file_path" in
  /var/home/jack/Code/koan/README.md|/var/home/jack/Code/koan/TUTORIAL.md|/var/home/jack/Code/koan/ROADMAP.md|/var/home/jack/Code/koan/design/*.md|/var/home/jack/Code/koan/roadmap/*.md)
    cat <<'JSON'
{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"This Edit/Write touches the koan doc tree (README.md, TUTORIAL.md, ROADMAP.md, design/, or roadmap/). If you have not already invoked the `documentation` skill this session, do so before continuing — it carries the partition rules and the doclinks workflow (problem-vs-impact partition, design-doc no-historical-narrative rule, and the doclinks check/deps/orphans gating triple)."}}
JSON
    ;;
esac

exit 0
