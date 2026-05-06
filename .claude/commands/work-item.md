---
description: Drive a roadmap item end-to-end — plan, approve, implement, update docs.
argument-hint: <roadmap-path>
---

You are orchestrating a koan roadmap item from plan to docs. The argument `$ARGUMENTS` is a path to a `roadmap/*.md` file.

Your job is procedural plumbing: read inputs, delegate to sub-agents, gate on user approval, run final audits. **Do not implement, plan, or write docs yourself.** Each phase belongs to a sub-agent.

## Approval gate template

Steps 3, 4, and 5 each end with a user approval gate. They all follow the same shape — apply this template with the inputs each step provides:

**Inputs per use:** `agent_output` (the verbatim returned text from the sub-agent), `accept_label` (what "Accept" advances to), `abort_consequence` (what state remains if the user aborts), `iterate_action` (which agent gets re-spawned and with what additional context).

**Procedure:**

1. Emit `agent_output` to the user as your user-facing text, complete and verbatim. Do not summarize, paraphrase, condense, bullet-ify, or extract "key picks." The user cannot see sub-agent output directly — your text is their only window into it. If it's long, emit it anyway. Summarizing here is a workflow failure.
2. In the same turn, call AskUserQuestion with three options:
   - **Accept** — `accept_label`.
   - **Iterate** — `iterate_action`. The option description must tell the user to type their feedback in the **notes** field on this option. Cap at 3 iterations; after that, ask whether to continue or abort.
   - **Abort** — `abort_consequence`.
3. When the response comes back, read `annotations[<question text>].notes` for the iterate feedback — that's the user's text-box input, no separate prompt needed. If Iterate is chosen with empty notes, ask once for the feedback before re-spawning (don't re-spawn with nothing).

Do **not** present an agent's output and ask "should I proceed?" via text — always use AskUserQuestion.

## Workflow

### Preflight: clean working tree

Before any other step, verify the git working tree is clean:

```bash
git status --porcelain
```

If output is non-empty, stop. Tell the user to commit or stash their changes, then re-run the command. Do not proceed — pre-existing changes would be mixed with the implementer's output, breaking the stash-on-abort flow in step 4 and confusing the audit in step 6.

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

Then apply the **Approval gate template** with:

- `agent_output` = the Plan agent's full returned text.
- `accept_label` = "proceed to implementation."
- `iterate_action` = "re-spawn Plan with the user's feedback appended."
- `abort_consequence` = "exit cleanly. Leave the plan file as a record."

### 4. Spawn the implementer agent

On Accept from step 3:

```
Agent(subagent_type=implementer, prompt=<approved plan + roadmap path>)
```

The agent returns a structured summary (Files changed, Design decisions, Caveats, Roadmap delta, Doc impact hint, Verification run).

Then apply the **Approval gate template** with:

- `agent_output` = the implementer's structured summary.
- `accept_label` = "proceed to doc-shepherd."
- `iterate_action` = "re-spawn implementer with the user's feedback appended."
- `abort_consequence` = "stash the code changes and stop. See **Stash on implementer abort** below."

**Stash on implementer abort.** When the user aborts at the implementer gate:

1. Derive `slug` = basename of `$ARGUMENTS` with the `.md` suffix stripped (e.g. `roadmap/module-system-1-module-language.md` → `module-system-1-module-language`).
2. Compute the next attempt index: `n = $(git stash list | grep -c "work-item:<slug>:") + 1`.
3. Run `git stash push -u -m "work-item:<slug>:<n>"` (the `-u` includes untracked files).
4. Report the resulting stash ref (`stash@{0}`) and the message tag back to the user so they can `git stash apply` later. If `git stash push` reports "No local changes to save," skip the stash and just report that the tree was already clean.

### 5. Spawn the doc-shepherd agent

On Accept from step 4:

```
Agent(subagent_type=doc-shepherd, prompt=<implementer summary + git diff main...HEAD + $ARGUMENTS>)
```

`git diff main...HEAD` is ground truth — pass it verbatim. The agent returns a list of doc edits applied + the doclinks gate state.

Then apply the **Approval gate template** with:

- `agent_output` = the doc-shepherd's returned text.
- `accept_label` = "proceed to the orchestrator audit (step 6)."
- `iterate_action` = "re-spawn doc-shepherd with the user's feedback appended."
- `abort_consequence` = "skip step 7 (Final disposition) and behave as if the user picked **Hold for review** — leave all changes uncommitted for the user to inspect. Do not stash."

### 6. Audit (you do this, not the sub-agents)

Don't trust either sub-agent's "tests pass" / "links resolve" claim. Re-run:

```bash
cargo test --quiet
python3 tools/doclinks.py check && python3 tools/doclinks.py deps && python3 tools/doclinks.py orphans
```

If anything fails, report the failure and stop. Don't try to fix it yourself — either re-spawn the relevant agent with the failure as input, or hand back to the user.

### 7. Final disposition

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
