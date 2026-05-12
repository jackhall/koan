---
description: Iteratively explore a refactor by spawning fresh explorer sub-agents against an evolving concept file.
argument-hint: <topic | empty>
---

You are running a refactor-exploration loop with the user. The point: each iteration spawns a **fresh** `explorer` sub-agent (no prior context) against a concept file you and the user write together. Rewriting the concept file between iterations strips out negative framing so the explorer is not anchored to what was already rejected.

`$ARGUMENTS` is an optional topic hint. Empty means open the conversation by asking what needs simplifying and what hints the user has.

This command does not write source code, run cargo, or commit. It only writes the concept file under `scratch/` and reports the explorer's proposals back to the user.

## Workflow

### 1. Write the initial concept

Write the user's initial concept and hints to `scratch/refactor-<slug>-concept.md` (short kebab-case slug from the topic). There is one concept file per session — it gets overwritten in place between iterations, not versioned. Show the user the file and let them iterate before continuing.

### 2. Spawn the explorer (fresh, every iteration)

Use the `Agent` tool with `subagent_type: explorer`. The prompt is short and self-contained:

> Explore `scratch/refactor-<slug>-concept.md`.

Never reuse a prior explorer via `SendMessage` — fresh context per iteration is the whole point. Do not paste the concept body into the prompt, and do not summarize prior iterations into the prompt; both re-anchor the agent.

### 3. Show output verbatim, then discuss

Emit the explorer's response **verbatim** to the user. Do not summarize, re-rank, or add commentary on top. Then talk with the user about it — what landed, what missed, what hints to refine, etc. Do not use AskUserQuestion.

### 4. Rewrite the concept in place

When the conversation reveals what should change for the next exploration, work with the user to overwrite `scratch/refactor-<slug>-concept.md`. Treat the rewrite as the actual lever:

- **Strip negative framing.** No "we tried X and it didn't work", "the explorer rejected Y", "approach Z is bad". The next explorer should not see what was dismissed — only what the current direction is.
- **Strip references to past iterations** so as not to anchor the next explorer.
- **Restate the goal** if the user's feedback shifted what "simpler" means.

The concept file is *not* a changelog. Do not include "rejected last iteration" or diffs from the prior version. Write it as if it were the only iteration. Do not limit the size of this file; it should develop more detail as we iterate. 

Get the user's approval for the new concept using AskUserQuestion, then loop back to step 2.

### 5. Stop when the user is ready to write a roadmap item

Iteration proceeds until the user says they are ready to write a roadmap item or that the refactor is a bad idea.

If the user is ready to write a roadmap item, write one with the locked direction of the conversation so far. If part(s) of the plan are still unclear, flag that to the user and give them a chance to resume iterating on the concept file.

If the users says the refactor is a bad idea, delete the concept file and exit clean. Do not invoke the `Plan` agent, `/work-item`, or any implementer.

## Constraints

- **Read-only** outside `scratch/`. No edits to `src/`, `roadmap/`, `design/`, `README.md`, `TUTORIAL.md`, `ROADMAP.md`, or `.claude/`.
- **No** `cargo` commands. **No** commits.
- Emit the explorer's output verbatim before discussing it, per the `feedback_subagent_output_verbatim` memory.
- **Do not do your own exploration**; that is the explorer's job.
