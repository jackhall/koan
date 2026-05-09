---
name: approval-gate
description: Reusable user-approval gate for orchestration skills that delegate to sub-agents. Defines the standard Accept / Abort / Iterate pattern — verbatim agent output, AskUserQuestion with two explicit options plus "Other" as the iterate channel, and a 3-cycle iterate cap.
---

# approval-gate

Reusable approval-gate template for orchestration skills (e.g. `work-item`) that hand work to sub-agents and need the user to approve each agent's output before advancing.

## When to apply

After a sub-agent returns and before advancing to the next step. The caller supplies four inputs per use:

- `agent_output` — the verbatim returned text from the sub-agent.
- `accept_label` — what "Accept" advances to (one phrase, e.g. "proceed to doc-shepherd").
- `abort_consequence` — what state remains if the user aborts (one phrase or short paragraph if cleanup is needed).
- `iterate_action` — which agent gets re-spawned and with what additional context (e.g. "re-spawn implementer with the user's feedback appended").

## Procedure

1. **Emit `agent_output` to the user as your user-facing text, complete and verbatim.** Do not summarize, paraphrase, condense, or bullet-ify. The user cannot see sub-agent output directly — your text is their only window into it.

2. **In the same turn, call AskUserQuestion with exactly two explicit options.** Iterate is the built-in "Other" channel, not its own option — AskUserQuestion always exposes "Other" with a free-text input, and adding an explicit "Iterate" alongside it splits the iterate path.
   - **Accept** — `accept_label`.
   - **Abort** — `abort_consequence`.

   The question text itself must point users at Other, e.g. "How do you want to proceed? (Pick Other to give feedback for another iteration.)"

3. **When the response comes back:**
   - "Accept" → advance to the next step.
   - "Abort" → run the abort consequence.
   - Otherwise (the user picked "Other" with custom text — or any other variant) → treat the response text as iterate feedback and re-spawn `iterate_action` with that feedback appended.

4. **Cap iterate cycles at 3.** After three iterate rounds, ask the user in plain text whether to continue iterating or abort.

## Allowed overrides

Callers may replace the second explicit option (Abort) with a domain-specific exit when the gate's context calls for it. Example: a plan-phase gate might use **Discuss language design** instead of **Abort**, with a custom consequence that exits to `/design`. The two-explicit-options-plus-Other-as-iterate shape is preserved either way.
