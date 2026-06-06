# Generalize `Scope::out` into monadic side-effect capture

**Problem.** [`Scope::out`](../../src/machine/core/scope.rs) is a `Box<dyn Write>` sink that
exists solely so [`PRINT`](../../src/builtins/print.rs) has somewhere to send bytes
and tests can swap stdout for a buffer. It is the only side-effect channel the runtime
has, and it is hard-coded to one channel and one shape (write bytes). Every additional
effect Koan eventually wants to support — file IO, time, randomness, network,
environment access, even error reporting — would either grow `Scope` by another ad-hoc
`Box<dyn ...>` field or get baked into `std::io` calls inside individual builtins.

Meanwhile the [`Monadic`](../../src/machine/model/types/ktraits.rs) trait already exists, with `pure` +
`bind` over a `Wrap<T>` GAT, and its doc comment says it is "intended as the abstraction
Koan's deferred-task and error-handling combinators will share once they're fleshed out."
Today it is implemented only for `Option` and threaded through nothing in the runtime. It
is scaffolding without a building.

**Acceptance criteria.**

- Koan code expresses every effect through one `Monad` signature
  ([design/effects.md](../../design/effects.md)), with each effect sharing
  that surface rather than a bespoke one.
- A test swapping the `Random`/`IO` module observes mocked output; a recorded
  trace replays identically.
- A function whose parameter list contains no `Monad`-kind module is
  referentially transparent, and the scheduler treats it as such.
- Module-system stage 4's generators thread randomness through the `Random`
  effect module.

**Directions.**

- *In-language `Monad` signature — decided per [design/effects.md](../../design/effects.md).*
  Implementation lands the signature plus `pure` / `bind`. The `Wrap`
  higher-kinded slot surface (`(TEMPLATE Type)` declaration form,
  `KType::ConstructorApply` application) has landed via module-system
  stage 2; see
  [design/typing/functors.md § Higher-kinded type slots](../../design/typing/functors.md#higher-kinded-type-slots).
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

Soft ordering: this is the third `BodyResult` revision sweeping every builtin in
[builtins/](../../src/builtins) (after TCO's `Value | Tail` and the error item's `Err`
arm), so fold it into the eventual static-typing/JIT pass if their schedules align.

**Requires:**

- [Standard library](standard-library.md) — the standard effect modules
  (`Random`, `IO`, `Time`) ship as stdlib entries.

**Unblocks:**

- [Module system stage 4 — Property testing and axioms](../predicate_typing/axioms-and-generators.md)
  — generators thread randomness via the `Random` effect module.
