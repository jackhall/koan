---
name: approval-gate
description: Reusable user-approval gate for orchestration skills that delegate to sub-agents. Defines the standard Accept / Abort / Iterate pattern — verbatim agent output, a plain-text gate with two explicit options plus free-text feedback as the iterate channel, and a 3-cycle iterate cap.
---

# approval-gate

Reusable approval-gate template for orchestration skills (e.g. `work-item`) that hand work to sub-agents and need the user to approve each agent's output before advancing.

## When to apply

After a sub-agent returns and before advancing to the next step. The caller supplies three inputs per use. When combined with the sub-agent output, the parameters for an approval gate are:

- `agent_output` — the verbatim returned text from the sub-agent.
- `accept_label` — what "Accept" advances to (one phrase, e.g. "proceed to shepherd").
- `abort_consequence` — what state remains if the user aborts (one phrase or short paragraph if cleanup is needed).
- `iterate_action` — which agent gets re-spawned and with what additional context (e.g. "re-spawn shepherd with the user's feedback appended").

## Procedure

1. **Write `agent_output` to a file in `scratch/`, complete and verbatim, and point the user at that file.** Do not summarize, paraphrase, condense, or bullet-ify. The user cannot see sub-agent output directly — your text is their only window into it.

2. **In the same turn, end the message with the gate in plain text — never through `AskUserQuestion` — and wait for the reply.** Offer exactly two explicit options, and say that any other reply is feedback for another iteration:
   - **Accept** — `accept_label`.
   - **Abort** — `abort_consequence`.
   - Or reply with feedback — `iterate_action`.

3. **When the response comes back:**
   - "Accept" → advance to the next step. If the sub-agent's work touched Rust source (an in-place refactor or other code-writing pass), run the `verify-koan` skill first so the advance lands on a build that has tests + clippy green and a recorded modgraph baseline.
   - "Abort" → run the abort consequence.
   - Anything else → treat the reply as iterate feedback and re-spawn `iterate_action` with that feedback appended.

4. **Cap iterate cycles at 3.** After three iterate rounds, ask the user in plain text whether to continue iterating or abort.

## Allowed overrides

Callers may replace the second explicit option (Abort) with a domain-specific exit when the gate's context calls for it. Example: a plan-phase gate might use **Discuss language design** instead of **Abort**, with a custom consequence that exits to `/design`. The two-explicit-options-plus-free-text-iterate shape is preserved either way.
