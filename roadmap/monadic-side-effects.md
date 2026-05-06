# Generalize `Scope::out` into monadic side-effect capture

**Problem.** [`Scope::out`](../src/dispatch/runtime/scope.rs) is a `Box<dyn Write>` sink that
exists solely so [`PRINT`](../src/dispatch/builtins/print.rs) has somewhere to send bytes
and tests can swap stdout for a buffer. It is the only side-effect channel the runtime
has, and it is hard-coded to one channel and one shape (write bytes). Every additional
effect Koan eventually wants to support — file IO, time, randomness, network,
environment access, even error reporting — would either grow `Scope` by another ad-hoc
`Box<dyn ...>` field or get baked into `std::io` calls inside individual builtins.

Meanwhile the [`Monadic`](../src/dispatch/types/ktraits.rs) trait already exists, with `pure` +
`bind` over a `Wrap<T>` GAT, and its doc comment says it is "intended as the abstraction
Koan's deferred-task and error-handling combinators will share once they're fleshed out."
Today it is implemented only for `Option` and threaded through nothing in the runtime. It
is scaffolding without a building.

**Impact.**

- *Uniform effect inspection.* One channel captures every kind of effect a builtin
  performs — file IO, time, randomness, network access — instead of each new effect
  requiring its own bespoke testing seam the way `PRINT`'s writer-swap does today.
- *Mocking and replay.* A test or replay handler feeds recorded effect results back to
  a program, making deterministic replay (feed a recorded trace, get the same output)
  mechanically possible.
- *Pure/effectful boundary.* The language can tell whether an expression is referentially
  transparent — unlocking memoization, reordering, and parallelism for the scheduler in
  cases where no effect is in play.
- *Explicit effect ordering.* Effects become declarative — "this expression's effect is
  X, sequenced after Y" — rather than dropping out of the scheduler's operational order.

**Directions.** None of these are decided.

- *Effect type.* Probably an enum: `Effect::Output(Vec<u8>)`, `Effect::Read(handle)`,
  `Effect::Now`, `Effect::Random`, plus a catch-all for builtins to declare custom
  effects. Open question: enumerated (closed set, easy to handle exhaustively) vs
  trait-object (`Box<dyn Effect>`, extensible by user code if/when user-defined functions
  can declare their own effects).
- *Carrier shape.* `BuiltinFn` returns not a bare `&'a KObject<'a>` (or `Result<...>`
  after the error-handling item) but an `Effectful<T>` carrier — a value paired with a
  list of pending effects. `Effectful` implements `Monadic`: `pure(v)` is `(v, [])`,
  `bind` concatenates effect lists. This is the long-promised second `Monadic` impl the
  trait's doc comment is waiting for.
- *Handler in `Scope`.* `Scope::out` becomes `Scope::handler: Box<dyn EffectHandler>`. The
  handler decides what to do with each `Effect` as the interpreter drains them: a default
  handler actually performs them (write to stdout, read the clock); a test handler
  captures them into a vec; a replay handler feeds results from a pre-recorded trace.
- *Drainage points.* Effects can either be performed eagerly (handler runs them as each
  builtin returns) or lazily (collected up the tree and run in batches at top-level
  expression boundaries). Eager is simpler and matches today's behavior; lazy unlocks
  reordering and is closer to the "monad transformer stack" shape this is converging on.
  Pick one explicitly rather than letting it emerge.

## Dependencies

No hard prerequisites; no remaining items downstream. Transient-node reclamation
(originally listed as downstream of this work) shipped independently — the
reclamation work was scheduler-internal and didn't need to share a `BuiltinFn`
signature pass.

`BodyResult` already absorbed one revision (`Value | Tail` for TCO); the error item added
a second (`Err` arm) and this one adds a third (`Effectful<...>`). Three churning passes
over every builtin in [builtins/](../src/dispatch/builtins/) is meaningfully worse than
one — but with reclamation already landed, the only remaining lever is folding effects
into the eventual static-typing/JIT pass if their schedules align.
