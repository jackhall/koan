# Inferred CLOSE

**Problem.** [CLOSE OVER](close-over.md) requires an explicit capture list;
the common case is "close over exactly what the block uses," which today
must be spelled out by hand.

**Acceptance criteria.**

- `CLOSE (block)` is observably equivalent to `CLOSE OVER (<free ids>)
  (block)`, including the memory-substrate severance the parent item tests.
- `$` anywhere lexically inside the block raises a structured error naming
  the inference conflict.
- Shadowing is respected: a name bound by a `LET` in the block, or a nested
  FN parameter, is not captured.

**Directions.**

- *Inference — decided per [design/lazy-closures.md](../../design/lazy-closures.md).*
  `CLOSE (block)` takes no capture list: the interpreter collects the
  block's free value identifiers (identifiers not bound by a binder form
  within the block, nested FN bodies included) and behaves as
  `CLOSE OVER (<those>) (block)`. Callables and modules already close
  implicitly, so inference is identifiers-only. `CLOSE OVER ()` (empty
  capture) and `CLOSE` (inferred) are distinct.
- *EVAL — decided.* Any `$(...)` lexically inside the block is a structured
  error: EVAL resolves names dynamically, so free names cannot be
  identified — explicit `CLOSE OVER` remains the EVAL-friendly form.

## Dependencies

**Requires:**

- [CLOSE OVER](close-over.md)

**Unblocks:** none.
