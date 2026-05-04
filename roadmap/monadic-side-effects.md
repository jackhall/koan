# Generalize `Scope::out` into monadic side-effect capture

**Problem.** [`Scope::out`](../src/dispatch/scope.rs) is a `Box<dyn Write>` sink that
exists solely so [`PRINT`](../src/dispatch/builtins/print.rs) has somewhere to send bytes
and tests can swap stdout for a buffer. It is the only side-effect channel the runtime
has, and it is hard-coded to one channel and one shape (write bytes). Every additional
effect Koan eventually wants to support — file IO, time, randomness, network,
environment access, even error reporting — would either grow `Scope` by another ad-hoc
`Box<dyn ...>` field or get baked into `std::io` calls inside individual builtins.

Meanwhile the [`Monadic`](../src/dispatch/ktraits.rs) trait already exists, with `pure` +
`bind` over a `Wrap<T>` GAT, and its doc comment says it is "intended as the abstraction
Koan's deferred-task and error-handling combinators will share once they're fleshed out."
Today it is implemented only for `Option` and threaded through nothing in the runtime. It
is scaffolding without a building.

**Impact.**

- *No effect inspection.* Tests can capture `PRINT` output by swapping the writer, but
  there is no equivalent for any other effect a builtin might want to perform. Each new
  effect requires its own bespoke testing seam.
- *No mocking or replay.* A program's behavior is whatever the host system decides at the
  moment of the call. Deterministic replay of a Koan program (feed it a recorded effect
  trace, get the same output) is impossible without a uniform effect channel.
- *No pure/effectful boundary.* The language has no way to know whether an expression is
  referentially transparent. Optimizations the scheduler could make (memoization,
  reordering, parallelism) are unsafe by default because any builtin might secretly write
  to a file or read the clock.
- *Effect ordering is implicit.* Today, effects happen in whatever order the scheduler
  runs builtins. There is no declarative "this expression's effect is X, sequenced after
  Y" — it is all operational.

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

**Unblocks:**
- [Transient-node reclamation](transient-node-reclamation.md)

`BodyResult` already absorbed one revision (`Value | Tail` for TCO); the error item added
a second (`Err` arm) and this one adds a third (`Effectful<...>`). Three churning passes
over every builtin in [builtins/](../src/dispatch/builtins/) is meaningfully worse than
one. Unless the effect story sharpens enough to fold into the same pass as ownership and
errors, this should land last and accept that the prior two items are stepping stones
rather than end states.
