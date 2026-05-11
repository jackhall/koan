# Generalize `Scope::out` into monadic side-effect capture

**Problem.** [`Scope::out`](../src/dispatch/runtime/scope.rs) is a `Box<dyn Write>` sink that
exists solely so [`PRINT`](../src/builtins/print.rs) has somewhere to send bytes
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

- *Uniform effect interface in-language.* Koan code expresses every effect through one
  `Monad` signature ([design/effects.md](../design/effects.md)) rather than each effect
  having its own bespoke surface.
- *Mocking and replay become a module swap.* A test feeds an alternate `Random` or `IO`
  module; deterministic replay (recorded trace, identical re-run) drops out
  mechanically.
- *Pure/effectful boundary is visible.* A function whose parameter list contains no
  `Monad`-kind module is referentially transparent — unlocking memoization, reordering,
  and parallelism for the scheduler in cases where no effect is in play.
- *Substrate for stage 4.* Module-system stage 4's generators thread randomness via the
  `Random` effect module; this work is what makes that possible.

**Directions.**

- *In-language `Monad` signature — decided per [design/effects.md](../design/effects.md).*
  Implementation lands the signature, the `Wrap` higher-kinded slot, and `pure` /
  `bind`. Requires module-system stage 2's functor support so `Wrap` can be a
  higher-kinded abstract type slot.
- *Standard effect modules — decided.* `Random`, `IO`, `Time`, plus existing
  `PRINT`-emitting builtins folded into `IO`. Each ascribes the `Monad` signature plus
  per-effect operations.
- *Runtime carrier — decided.* `BuiltinFn` returns an `Effectful<T>` carrier — a value
  paired with pending effects. `Effectful` is the second `Monadic` impl the trait's doc
  comment is waiting for, and it bridges the in-language `Monad` signature and the
  runtime's effect drainage path.
- *Handler in `Scope` — decided.* `Scope::out` becomes
  `Scope::handler: Box<dyn EffectHandler>`. Handlers decide what to do with each pending
  `Effect`: default performs them, test captures them into a vec, replay feeds from a
  pre-recorded trace.
- *Drainage points — open.* Eager (handler runs effects as each builtin returns) or lazy
  (collected up the tree, run at top-level boundaries). Eager is simpler; lazy unlocks
  reordering. Pick one explicitly.

## Dependencies

**Requires:**
- [Module system stage 2 — Module values and functors through the scheduler](module-system-2-scheduler.md)
  — the in-language `Monad` signature's `Wrap` slot is a higher-kinded
  abstract type, expressible only with functor support.
- [Standard library](standard-library.md) — the standard effect modules
  (`Random`, `IO`, `Time`) ship as stdlib entries.

**Unblocks:**
- [Module system stage 4 — Property testing and axioms](module-system-4-axioms-and-generators.md)
  — generators thread randomness via the `Random` effect module.

`BodyResult` already absorbed one revision (`Value | Tail` for TCO); the error item added
a second (`Err` arm) and this one adds a third (`Effectful<...>`). Three churning passes
over every builtin in [builtins/](../src/builtins/) is meaningfully worse than
one — but with reclamation already landed, the only remaining lever is folding effects
into the eventual static-typing/JIT pass if their schedules align.
