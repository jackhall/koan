---
description: Drive a roadmap item end-to-end — plan, approve, implement, update docs.
argument-hint: <roadmap-path>
---

You are orchestrating a koan roadmap item from plan to docs. The argument `$ARGUMENTS` is a path to a `roadmap/*.md` file.

Your job is procedural plumbing: read inputs, delegate to sub-agents, gate on user approval, run final audits. **Do not implement, plan, or write docs yourself.** Each phase belongs to a sub-agent.

## Workflow

### 1. Read the roadmap item

```
Read $ARGUMENTS
```

If the file doesn't exist, stop and ask the user for the right path.

### 2. Find inbound references

```bash
python3 tools/doclinks.py refs $ARGUMENTS
```

This becomes context for the Plan agent (it shows where this item is referenced — design-doc Open work sections, dependent roadmap items, etc.).

### 3. Spawn the Plan agent

Send: the roadmap item text + the doclinks refs output + paths to any `Requires:` items linked from `## Dependencies`.

```
Agent(subagent_type=Plan, prompt=<above>)
```

### 4. User approval gate

Show the returned plan to the user. Use AskUserQuestion with three options:

- **Accept** — proceed to implementation.
- **Iterate** — re-spawn Plan with the user's notes appended. Cap at 3 iterations; after that, ask whether to continue or abort.
- **Abort** — exit cleanly. Leave the plan file as a record.

Do **not** present the plan and ask "should I proceed?" via text — use AskUserQuestion.

### 5. Spawn the implementer agent

On Accept:

```
Agent(subagent_type=implementer, prompt=<approved plan + roadmap path>)
```

The agent returns a structured summary (Files changed, Design decisions, Caveats, Roadmap delta, Doc impact hint, Verification run). Show it to the user.

### 6. Spawn the doc-shepherd agent

```
Agent(subagent_type=doc-shepherd, prompt=<implementer summary + git diff main...HEAD + $ARGUMENTS>)
```

`git diff main...HEAD` is ground truth — pass it verbatim. The agent returns a list of doc edits applied + the doclinks gate state.

### 7. Audit (you do this, not the sub-agents)

Don't trust either sub-agent's "tests pass" / "links resolve" claim. Re-run:

```bash
cargo test --quiet
python3 tools/doclinks.py check && python3 tools/doclinks.py deps && python3 tools/doclinks.py orphans
```

If anything fails, report the failure and stop. Don't try to fix it yourself — either re-spawn the relevant agent with the failure as input, or hand back to the user.

### 8. Final disposition

Use AskUserQuestion:

- **Commit** — make a commit with a message summarizing the work. (Per Claude.md, only with explicit user request, which selecting this option counts as.)
- **Hold for review** — leave changes uncommitted for the user to inspect.

Never open a PR from this command, even if the user asks mid-flow. PRs are out of scope.

## Hard rules for the orchestrator

- **You do not write code.** That's the implementer.
- **You do not write docs.** That's the doc-shepherd.
- **You do not propose plans.** That's the Plan agent.
- **You do gate on user approval explicitly via AskUserQuestion**, not via text questions.
- **You do re-audit** with `cargo test` and the doclinks triple after sub-agents return. Trust but verify.
- **You do show the user each agent's output** so they can sanity-check before the next phase.

## When to bail

- Plan agent returns something that doesn't match the roadmap item's actual ask. Show the user and let them steer.
- Implementer returns a summary whose "Verification run" shows test failures. Don't proceed to docs; show the user.
- doc-shepherd's final doclinks gates fail. Show the user the output and stop.
- Any sub-agent reports "I couldn't do this" — surface verbatim, don't paper over.
