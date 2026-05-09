---
name: work-item
description: Trigger as soon as a plan is ready for a change tracked in `roadmap/` (e.g. user says the plan is approved, or hands over a plan file with a roadmap path). Drives the item from plan to shipped docs — implement, doc-shepherd, audit, dispose. Skips planning.
---

# work-item

You are orchestrating a koan roadmap item from an already-approved plan to docs. The skill takes two paths as input:

- `<roadmap-path>` — a `roadmap/*.md` file describing the item.
- `<plan-path>` — a file containing the implementation plan the user has already prepared (e.g. via `/design`, prior conversation, or by hand).

Both paths come from the skill's args. If either is missing or the file doesn't exist, stop and ask the user for the right paths.

Your job is procedural plumbing: read inputs, delegate to sub-agents, gate on user approval, run final audits. **Do not implement or write docs yourself.** Each phase belongs to a sub-agent.

Steps 2 and 3 use the shared **approval-gate** skill — invoke it with the per-step inputs given below.

## Workflow

### Preflight: clean working tree (docs allowed) + read inputs

Verify the git working tree has no dirty *non-doc* paths:

```bash
git status --porcelain \
  | grep -vE ' (README\.md|TUTORIAL\.md|ROADMAP\.md|design/[^ ]+\.md|roadmap/[^ ]+\.md)$'
```

If output is non-empty, stop. Tell the user to commit or stash their non-doc changes, then re-invoke. Pre-existing source changes would be mixed with the implementer's output, breaking the stash-on-abort flow in step 2 and confusing the audit in step 4. Pre-existing doc changes are fine — the implementer doesn't touch docs, and the doc-shepherd's edits will simply commingle with them (the user's intent when they left docs dirty).

Then read both `<roadmap-path>` and `<plan-path>`. If either is missing, stop and ask.

### 1. Spawn the implementer agent

```
Agent(subagent_type=implementer, prompt=<plan contents + roadmap path>)
```

The agent returns a structured summary (Files changed, Design decisions, Caveats, Roadmap delta, Doc impact hint, Verification run).

### 2. Implementer approval gate

Apply the **approval-gate** skill with:

- `agent_output` = the implementer's structured summary.
- `accept_label` = "proceed to doc-shepherd."
- `iterate_action` = "re-spawn implementer with the user's feedback appended."
- `abort_consequence` = "stash the code changes and stop. See **Stash on implementer abort** below."

**Stash on implementer abort.** When the user aborts at the implementer gate:

1. Derive `slug` = basename of `<roadmap-path>` with the `.md` suffix stripped (e.g. `roadmap/module-system-1-module-language.md` → `module-system-1-module-language`).
2. Compute the next attempt index: `n = $(git stash list | grep -c "work-item:<slug>:") + 1`.
3. Run `git stash push -u -m "work-item:<slug>:<n>"` (the `-u` includes untracked files). Note: this also stashes any pre-existing doc changes the preflight allowed through. The stash is the user's restore point either way.
4. Report the resulting stash ref (`stash@{0}`) and the message tag back to the user so they can `git stash apply` later. If `git stash push` reports "No local changes to save," skip the stash and just report that the tree was already clean.

### 3. Spawn the doc-shepherd agent

On Accept from step 2:

```
Agent(subagent_type=doc-shepherd, prompt=<implementer summary + git diff main...HEAD + roadmap-path>)
```

`git diff main...HEAD` is ground truth — pass it verbatim. The agent returns a list of doc edits applied + the `doclinks check` state.

Then apply the **approval-gate** skill with:

- `agent_output` = the doc-shepherd's returned text.
- `accept_label` = "proceed to the orchestrator audit (step 4)."
- `iterate_action` = "re-spawn doc-shepherd with the user's feedback appended."
- `abort_consequence` = "skip step 5 (Final disposition) and behave as if the user picked **Hold for review** — leave all changes uncommitted for the user to inspect. Do not stash."

### 4. Audit (you do this, not the sub-agents)

Don't trust either sub-agent's "tests pass" / "links resolve" claim. Re-run:

```bash
cargo test --quiet
python3 tools/doclinks.py check
```

If anything fails (cargo tests, or any of the three gating sections of `doclinks check`), report the failure and stop. Don't try to fix it yourself — either re-spawn the relevant agent with the failure as input, or hand back to the user. The source-tree-changes section of `check` is informational and never fails the gate; it's there as decision input for the doc-shepherd.

### 5. Final disposition

Use AskUserQuestion:

- **Commit** — make a commit with a message summarizing the work. (Per Claude.md, only with explicit user request, which selecting this option counts as.)
- **Hold for review** — leave changes uncommitted for the user to inspect.

Never open a PR from this skill, even if the user asks mid-flow. PRs are out of scope.

## When to bail

- Plan file describes something that doesn't match the roadmap item's actual ask. Show the user and let them steer.
- Implementer's "Verification run" shows test failures. Don't proceed to docs; show the user.
- doc-shepherd's final `doclinks check` returns non-zero. Show the user the output and stop.
- Any sub-agent reports "I couldn't do this" — surface verbatim, don't paper over.
