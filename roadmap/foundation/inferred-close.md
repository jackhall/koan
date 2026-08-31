# Inferred CLOSE

**Problem.** [CLOSE OVER](../../design/lazy-closures.md) requires an
explicit capture list; the common case is "close over exactly what the block
uses," which today must be spelled out by hand.

**Acceptance criteria.**

- `CLOSE (block)` is observably equivalent to `CLOSE OVER (<derived>)
  (block)` — including the memory-substrate severance the parent item
  tests — where `<derived>` is the block's free identifiers, both channels,
  position-aware (binder forms and nested FN parameters respected; quote
  bodies and severed sub-blocks excluded) that resolve in the per-call
  portion of the enclosing chain.
- A free identifier resolving nowhere raises `UnboundName` at the CLOSE
  statement; one resolving only in the eternal tier is read through the
  block's outer link, not captured.
- `$(...)` (or spelled `EVAL`) in the block's inference domain raises a
  structured error naming the inference conflict; nested `CLOSE OVER`
  blocks and quote bodies are exempt.
- `USING … SCOPE` in the inference domain raises the same structured error,
  with the same exemptions.
- Shadowing is respected: a name bound by a `LET` in the block, or a nested
  FN parameter, is not captured — and a use lexically *before* a `LET` of
  the same name is free, mirroring the strict `idx < cutoff` rule.

**Directions.**

- *Inference — decided per [design/lazy-closures.md](../../design/lazy-closures.md).*
  `CLOSE (block)` takes no capture list: the interpreter collects the
  block's free value identifiers (identifiers not bound by a binder form
  within the block, nested FN bodies included) and behaves as
  `CLOSE OVER (<those>) (block)`. Callables and modules already close
  implicitly, so inference is identifiers-only. `CLOSE OVER ()` (empty
  capture) and `CLOSE` (inferred) are distinct.
- *Capture selection — decided.* A free identifier resolving in the
  per-call portion of the chain is captured (an in-flight claim parks);
  eternal-only names are skipped — visible through the outer link, a
  capture would only add a copy; a name resolving nowhere is an
  `UnboundName` error at the form, matching explicit `CLOSE OVER`.
- *Type channel — decided.* Free *type* identifiers are inferred too and
  captured as type handles, since implicit close covers registrations and
  modules but not type bindings.
- *EVAL — decided.* Any `$(...)` in the inference domain is a structured
  error: EVAL resolves names dynamically, so free names cannot be
  identified — explicit `CLOSE OVER` remains the EVAL-friendly form.
- *USING — decided.* Any `USING … SCOPE` window in the inference domain is
  the same structured error: the window surfaces its module's members at run
  time, so a syntactic walk cannot tell a member reference from a free name —
  explicit `CLOSE OVER` remains the form that admits windows.
- *Inference domain — decided.* The walked region of the block: quote
  bodies are excluded as data, and the blocks of nested `CLOSE OVER` /
  `CLOSE` forms are excluded as severed (a nested `CLOSE` polices its own
  block when it evaluates).
- *Walk timing — decided.* The free-identifier walk runs per evaluation on
  the step scratch, not as a parse-time cache: CLOSE already re-freezes the
  block per evaluation, so a cache does not change the complexity class,
  and it would grow `KExpression` for every program. Revisit only with
  profiling evidence (see `scratch/inferred-close-plan.md` while the item
  is in flight).

## Dependencies

**Requires:**


**Unblocks:** none.
