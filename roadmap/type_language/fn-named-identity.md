# FN/FUNCTOR named identity

Round-trip parameter names from the sigil surface through
`KType::KFunction` / `KType::KFunctor` identity, built on the shared record
substrate.

**Problem.** The
[type-language-via-dispatch](../../design/typing/type-language-via-dispatch.md)
substrate ships the `:(FN (x :Number, y :Str) -> Bool)` and
`:(FUNCTOR (T :SomeSig) -> Module)` sigil surfaces, which declare parameter
names at the type position. Lowering drops the names:
[`KType::KFunction { args, ret }`](../../src/machine/model/types/ktype.rs)
and `KType::KFunctor { params, ret }` store `args` / `params` as a bare
`Vec<KType>`. The names are not missing upstream — every
[`Argument`](../../src/machine/model/types/signature.rs) slot carries a
`name` that keys it in the `ArgumentBundle` — they are discarded only at the
`KType` lowering. So a function-typed slot has no record of which names the
callee expects, and the use-site constraint (koan has no positional call
syntax; [execution-model.md](../../design/execution-model.md)) can't be
checked against the slot's type.

**Impact.**

- `KType::KFunction` / `KFunctor` carry their parameter record (the
  `(name, type)` pairs the signature already holds), so a function-typed
  slot records which names the callee expects.
- `KType::name()` round-trips: `:(FN (x :Number, y :Str) -> Bool)` renders
  with names and re-parses to the same `KType`.
- Function- and functor-typed slot identity is the record substrate's
  equality — same parameters by name and type, order-blind.

**Directions.**

- *Parameter record — decided.* The arg/param lists become the
  [record substrate](record-substrate.md)'s shape — an ordered
  `(name, KType)` map — replacing today's `Vec<KType>`. Type-only walks read
  the value projection; name-aware lookups read the pair.
- *Where the names come from — decided.* Build the parameter record from the
  signature's `Argument.name` + `Argument.ktype` slots. Keywords stay the
  dispatch bucket key; the parameter record is the typed slots only.
- *Builtin / FFI carriers — decided: no synthetic names, no wildcards.*
  Builtins already name their parameters at the `Argument` level; the lossy
  `KType` was the only thing dropping them. Propagating the names removes the
  need for placeholder/wildcard names entirely.
- *Rendered-form test migration — open.* Tests asserting names-absent
  rendering (`assert_eq!(t.name(), ":(FN (Number Str) -> Bool)")` in
  `ktype.rs`) and every `KType::KFunction` / `KFunctor` comparison site need
  re-checking against the named form. Scoped by a grep for
  `KType::KFunction` / `KFunctor` / the rendered `(FN …)` / `(FUNCTOR …)`
  forms.

## Dependencies

**Requires:**

- [Record substrate for identifier-keyed binding](record-substrate.md) —
  parameter identity is the substrate's order-blind `(name, type)` equality.

**Unblocks:**

- [Record structural subtyping and projection](record-subtyping.md) —
  width/depth admission over function-parameter records needs the names
  present in the `KType` first.
