# Effects

Effects as monadic values labelled on a function's type, composite effects
implemented by handlers, and a module bound from an action as a leaf.

**Problem.** The rewritten stack tracks no effect. A function's type says
nothing of what its body performs, so the load cannot tell a pure application
from an effectful one, and the [module design](../../design/modules.md)
unfolds every call: a functor that drew a fresh value would be read at load as
building one content at every run.

**Acceptance criteria.**

- A function type carries an effect row naming the effects its body may
  perform, and the load refuses a body that performs an effect its row leaves
  out.
- Performing an effect builds an action, and nothing is performed until a
  handler runs it.
- A composite effect is an effect of its own: `applog` declares its operations,
  such as `LOG`, and a handler implements them by performing file, stdout and
  network effects. A function whose row names `applog` names none of those.
- A test handler for `applog` captures the lines a program logs, with no change
  to the program.
- A call whose function's row names an effect does not
  [unfold](../../design/modules.md#a-call-unfolds), and a module bound by
  running the action it builds is a leaf: a functor that captures a value drawn
  from an effect builds a module of new carriers at each run of that action, and
  one that only logs while it builds gives the same carriers at each run.

**Directions.**

- *Effects labelled by a row — decided.* A function type names the set of
  effects its body may perform, so effects compose by union rather than by a
  stack of monads.
- *A composite effect — decided.* `applog` is an effect with operations of its
  own, which a handler implements in terms of other effects, and never an alias
  for a row: callers depend on `applog` alone, and a test or a new transport
  swaps the handler.
- *Effect polymorphism — open.* Whether a higher-order function is polymorphic
  in its argument's row, so one `MAP` serves pure and effectful callers.

## Dependencies

**Requires:**

- [Families as parameters](families-as-parameters.md) — a `Monad` signature
  over a family.
- [Path types](path-types.md) — an effectful call stays folded only where
  calls unfold.

**Unblocks:** none — a leaf.
