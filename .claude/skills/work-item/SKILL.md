---
name: work-item
description: Trigger for a change tracked in `roadmap/` — when the user hands over a roadmap path (with or without a plan file). Drives the item from plan to shipped docs — plan if needed, branch, implement inline yourself, then an adversarial shepherd audit-and-doc pass, then commit on approval.
---

# work-item

You are driving a koan roadmap item from plan to shipped docs. Unless told otherwise, do the implementation yourself, inline. If you encounter a large-scale but mechanical change, ask the user conversationally if you may fan out agents for it. 

Delegate final design/ and roadmap/ documentation and the final audit to the adversarial **shepherd** subagent. You may only edit markdown while you are in plan mode.

The skill takes one required path and one optional path, both in scratch and named after the roadmap item:

- `<roadmap-path>` — a `roadmap/*.md` file describing the item. **Required.**
- `<plan-path>` — a `scratch/*-plan.md` file containing an implementation plan the user prepared (e.g. via `/design`, prior conversation, or plan mode). **Optional.** If absent, you start in plan mode and produce one before touching code.

Throughout, `slug` = basename of `<roadmap-path>` with the `.md` suffix stripped (e.g. `roadmap/type_language/types-in-value-channel.md` → `types-in-value-channel`).

## Workflow

### Read inputs

Read `<roadmap-path>`. If a `<plan-path>` was given, read it too and sanity-check it matches the roadmap item's actual ask (bail per **When to bail** if it doesn't). 

### 1. Plan — only if no plan file was given

If a `<plan-path>` was provided, skip this step.

Otherwise, **start in plan mode** (`EnterPlanMode`). Research the item against the codebase and the relevant `design/*.md`, then present an implementation plan with `ExitPlanMode`. On approval, persist the approved plan to `scratch/<slug>-plan.md` (gitignored) so the shepherd and any re-invocation can read it, and bring the roadmap item up-to-date. Then proceed to the preflight check.

Planning runs first because it only edits markdown (in plan mode), so it needs no clean working tree — the clean-tree gate guards the *implementation* diff, so it sits right before you branch and write code.

### Preflight: clean working tree (docs allowed)

Sanity-check the scope, and flag large-but-decomposable scope to the user, who may opt to split up the roadmap item.

With a plan in hand and implementation about to start, verify the git working tree has no dirty *non-doc* paths:

```bash
git status --porcelain \
  | grep -vE ' (README\.md|TUTORIAL\.md|design/[^ ]+\.md|roadmap/[^ ]+\.md)$'
```

`scratch/` is .gitignored.

If output is non-empty, stop. Tell the user to commit or stash their non-doc changes, then re-invoke. A pre-existing source change would pollute the `git diff master...HEAD` the shepherd audits against and the verify slate it re-runs (step 3). Pre-existing doc changes are fine: the shepherd's edits simply commingle with them (the user's intent when they left docs dirty).

### 2. Branch, then implement — you do this, inline

**Cut the work branch first.** Now that a plan is ready and implementation is about to start, branch before the first code edit:

```bash
git checkout -b <slug>        # or: git checkout <slug>, if it already exists from a prior attempt
```

The branch is named after the roadmap item (`slug`), and all implementation lands here — not on the branch you started from. It forks from your current `HEAD`, so the `master...HEAD` diff the shepherd audits and verifies (step 3) covers everything since `master`; if you started somewhere other than `master`, that upstream work rides along in the diff — branch from `master` instead when you want the audit scoped to this item alone.

Then implement the plan directly against the codebase:

- Respect the plan. Use `ToDoWrite` to keep yourself on track. Run `cargo test --quiet` after each step to keep the suite green, and fix regressions before moving on.
- If you discover the plan is wrong mid-implementation, **surface it and stop** — don't silently re-design. The user may ask to return to planning.
- Use the `rust-refactor` skill for structural work (renames, file moves, batch rewrites). Don't reinvent its tooling.
- Use the `miri` skill whenever the work touches memory safety.
- Update top-of-file and inline source comments as you go, per Claude.md. **Don't** touch `design/`, `roadmap/` (including its `README.md` index), `README.md`, or `TUTORIAL.md` — those are for planning in step 1 or the shepherd in step 3.
- When code-complete, run the `verify-koan` skill so tests + clippy are green and a modgraph baseline is recorded before you hand off to the shepherd (which runs the authoritative final slate after its doc edits).

Your implementation is visible inline as you work, so there is no formal **approval gate** here. If you hit a fork that's genuinely the user's call, raise it conversationally in the moment — don't batch it into a gate.

If you hit something you genuinely can't do — surface it verbatim, don't paper over.

### 3. shepherd — adversarial audit, doc updates, and the final verify slate

Compose the structured summary below, then hand it plus the diff to the shepherd. The shepherd is **antagonistic to your implementation**: it independently verifies your claims against the diff before writing any docs, and it **owns the final audit** — after its doc edits it runs the full verify slate (`tools/verify.sh`: tests, clippy, `doclinks check`) as the authoritative green-light, so you do not re-run it yourself.

```
Agent(subagent_type=shepherd, prompt=<structured summary + git diff master...HEAD + roadmap-path>)
```

`git diff master...HEAD` is ground truth — pass it verbatim. The shepherd writes its report — audit findings, doc edits, and the final verify-slate result — to `scratch/<slug>-result.md` and returns that path plus a one-line status.

Structured summary you compose (this is the shepherd's input contract — match the shape):

```
## Files changed
- path/to/file.rs: <one-line summary of what changed and why>

## Design decisions
- <decision>: chose X over Y because <reason>. Trade-off: <what we give up>.

## Caveats
- <open follow-up>: <why it's punted, what would close it>   (or "none")

## Roadmap delta
- Completes: <roadmap/item.md path, or "none">.
- Should be deleted from roadmap/: <yes/no, with one-line reason>.
- New items surfaced: <list, or "none">.

## Doc impact hint
- design/<file>.md: <which sections plausibly need updating>   (or "none")

## Verification run
cargo test: <N passed, M failed>
clippy: <clean/issues>
```

Then apply the **approval-gate** skill — this is the work item's disposition gate, so **Accept commits and Abort holds**:

- `agent_output` = the shepherd's report. It already wrote the report to `scratch/<slug>-result.md` — **point the user at that file rather than re-writing it** (this satisfies the approval-gate "write to scratch and point the user there" step without a redundant write).
- `accept_label` = "commit the work item — selecting Accept authorizes the commit, so make one with a message summarizing the work."
- `iterate_action` = "either re-spawn the shepherd (for doc changes), return to implementation (for code changes), or return to planning (for design or scope changes)"
- `abort_consequence` = "leave all changes uncommitted for the user to inspect (the hold-for-review path). Do not stash."

On **Accept**, make the commit (message summarizing the work); selecting Accept is the explicit per-commit authorization Claude.md requires. Never open a PR from this skill, even if the user asks mid-flow — PRs are out of scope.

**Only offer Accept when the shepherd returns green or if the user has approved a yellow in conversation.** If the shepherd returns red for any reason (failed verify, scope mismatch, etc.), don't present the approval gate. Instead, explain the situation to the user and ask for guidance. The slate must be green before the commit.
