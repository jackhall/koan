# Effects

Effects as monadic values labelled on a function's type, composite effects
implemented by handlers, and a path through an application.

**Problem.** The rewritten stack tracks no effect. A function's type says
nothing of what its body performs, so the load cannot tell a pure application
from an effectful one, and the [module design](../../design/modules.md) has no
way to read `(MAKESET m).Set` as a path.

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
- An application of a function to paths is a path where the module it returns
  captures, through its `OVER` list, no value an effect produced:
  `(MAKESET m).Set` written at two sites over one `m` is one type at load.
- A functor that captures a value drawn from an effect builds a module of new
  carriers at each run of that action, and one that only logs while it builds
  gives the same carriers at each run.

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

**Unblocks:** none — a leaf.
