# Effects

Side effects in Koan — randomness, IO, time, network access, and others —
are surfaced as **in-language monads**. A `Monad` signature in Koan
declares the interface; concrete effect modules (`Random`, `IO`, `Time`,
…) ascribe it. Builtins that perform effects do so by calling into one of
these modules rather than touching ambient runtime state.

The motivation is uniformity: every effect Koan eventually wants — file
IO, time, randomness, network, environment access — lives behind one
interface. A single mocking story (swap the module) replaces a per-effect
testing seam. The pure/effectful boundary becomes visible to the type
system rather than implicit in builtin internals.

## Monad signature

```
SIG Monad = (
  (LET Wrap = ...)        -- type constructor: T → Wrap<T>
  (LET pure = (FN (PURE x: Any) -> Wrap<Any> = ...))
  (LET bind = (FN (BIND m: Wrap<Any> f: Function<(Any) -> Wrap<Any>>) -> Wrap<Any> = ...))
)
```

The `Wrap` slot is a type constructor — a signature with a higher-kinded
abstract type. Module-system stage 2's functor support is what makes this
expressible; without it, `Wrap` cannot be a parameterized type slot. This
is the dependency line between effects and module-system stage 2.

## Standard effect modules

Each Koan-level effect is a structure ascribing `Monad` plus per-effect
operations:

- **`Random`** — produces values from a random stream. Generators in
  module-system stage 4 take `Random` as an explicit parameter (until
  stage 5 makes it implicit).
- **`IO`** — read/write byte streams. Replaces the
  [`Scope::out`](../src/dispatch/runtime/scope.rs) `Box<dyn Write>` channel.
- **`Time`** — clock reads.
- *(others as the language grows)* — file IO, network, environment.

Each effect module exposes operations in the shape its semantics demand
(`Random.draw`, `IO.read`, `IO.write`, `Time.now`) on top of the inherited
`pure` and `bind`.

## Threading

Until modular implicits ship (module-system stage 5), effect-using FNs
take their effect module as an explicit parameter. The signature declares
the dependency at the FN's parameter list; the call site supplies the
module:

```
LET gen = (FN (GEN r: Random) -> Number = (... r ...))
```

Stage 5's implicit dispatch elides the parameter at call sites where the
effect is in scope. Until then, threading is verbose but coherent — every
effectful path names its effects.

## Builtin effects

Builtins that today emit side effects (`PRINT`, eventually `RANDOM`,
`NOW`, …) become callers of the corresponding effect module rather than
direct accessors of ambient state. The runtime drains effects through a
single channel: a default handler performs them; a test handler captures
them; a replay handler feeds recorded results.

The Rust-side
[`Monadic`](../src/dispatch/types/ktraits.rs) trait, currently scaffolding
for `Option`, becomes the bridge between the in-language `Monad`
signature and the runtime's effect drainage. It implements the carrier
shape the runtime uses for `Effectful<T>` — a value paired with pending
effects.

## Pure / effectful boundary

A function whose parameter list contains no `Monad`-kind module is
referentially transparent. The static-typing pass can detect this from
the function's signature alone. This unlocks memoization, reordering, and
parallelism for the scheduler in cases where no effect is in play.

The boundary is structural: an effectful FN that wants to remain pure-from
-its-callers'-perspective can be wrapped in a thunk constructor that
captures and discharges the effect privately. The type system tracks the
wrapper's purity, not the inner effectful body.

## Open work

- [Generalize `Scope::out` into monadic side-effect capture](../roadmap/monadic-side-effects.md)
  — the implementation work to ship the `Monad` signature, the standard
  effect modules, and the runtime drainage path.
