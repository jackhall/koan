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
  (TYPE (Type AS Wrap))          -- type constructor: applied as :(Number AS Wrap)
  (EXPR (PURE Elt :Type x :Elt) -> :(Elt AS Wrap))
  (EXPR (BIND Elt :Type Res :Type
          m :(Elt AS Wrap)
          f :(FN :{x :Elt} -> :(Res AS Wrap))) -> :(Res AS Wrap))
)
```

`Monad` is **one** signature, not a family of per-element ones. Three pieces
make it so.

**A type-constructor slot.** `TYPE (Type AS Wrap)` declares the wrapper,
applied as `:(Number AS Wrap)`; the higher-kinded surface form lives in
[typing/functors.md § Higher-kinded type slots](typing/functors.md#higher-kinded-type-slots).
Opaque ascription mints a per-call `SetMember` handle for a
`TypeConstructor`-kind member under the ascribed module's
`type_members[Wrap]`, so different `Monad`-ascribed modules carry distinct
`Wrap` identities.

**Quantified members.** An ordinary member names concrete types:
`(EXPR (PURE x :Number) -> :(Number AS Wrap))` declares one operation at one
element type. A **quantified** member binds its own type parameters and
declares the operation at *every* choice of them — `Elt :Type` above is the
**quantifier**, and its scope is the rest of the member's element sequence
and the return, both of which read the name it binds. `bind` quantifies twice
because it changes the element type: an `Elt` in, a `Res` out, one `Wrap`
throughout. A module satisfies a quantified member by supplying a single
implementation that holds at every instantiation — not one per element type —
so a module ascribes `Monad` once and is a monad at every element type. The
quantifier list is part of the shape's type; argument names are not.
Quantification is not a functor's `:Type` parameter, which is bound and
solved per call: a quantified member's parameters are solved once, at
satisfaction, against the candidate module's own overloads.

**Keyworded members, not `VAL` slots.** `PURE` and `BIND` are dispatch keys a
satisfying module answers to, checked by the same most-specific selection
dispatch runs
([typing/modules.md § Keyworded members](typing/modules.md#keyworded-members)).
A `VAL` slot holds a closed function type and could not carry a quantifier.

## Standard effect modules

Each Koan-level effect is a structure ascribing `Monad` plus per-effect
operations:

- **`Random`** — produces values from a random stream. Generators in
  module-system stage 4 take `Random` as an explicit parameter (until
  stage 5 makes it implicit).
- **`IO`** — read/write byte streams. Replaces the run frame's
  [`RunWriter`](../src/machine/core/arena/frame.rs) `Box<dyn Write>` channel,
  and gives a failed write a result for Koan code to read.
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
LET gen = FN (GEN r :Random) -> Number = (... r ...)
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

The runtime carrier that bridges the in-language signature and that drainage
is a value paired with the effects it has pending. A builtin that performs an
effect names it on the carrier it returns; the handler on the run frame
decides what performing it means.

## Pure / effectful boundary

A function whose parameter list names no `Monad`-ascribing module is
referentially transparent. The verdict is a read of the function's own
parameter record — does any slot's declared type satisfy `Monad`? — so it is
available from the signature alone, with no analysis of the body. This unlocks memoization, reordering, and
parallelism for the scheduler in cases where no effect is in play.

The boundary is structural: an effectful FN that wants to remain pure-from
-its-callers'-perspective can be wrapped in a thunk constructor that
captures and discharges the effect privately. The type system tracks the
wrapper's purity, not the inner effectful body.

## Open work

- [Monadic side effects](../roadmap/foundation/monadic-side-effects.md)
  — the implementation work: the member quantification the `Monad` signature
  needs, the standard effect modules, and the runtime drainage path.
