# Merge the raw-type-part slot markers

Collapse the two slot-only lazy-capture `KType` variants `SigiledTypeExpr` and
`RecordType` into one `RawTypePart(TypePartKind)` variant. Leave `KExpression` alone.

**Problem.** `KType` carries three "lazy slot" variants that each capture one unevaluated
[`ExpressionPart`](../../src/machine/model/ast.rs) raw via `resolve_for` so a declarator
owns its elaboration: [`KExpression`, `SigiledTypeExpr`, and
`RecordType`](../../src/machine/model/types/ktype.rs). Two of them —
`SigiledTypeExpr` (captures a `:(…)` type expr) and `RecordType` (captures a `:{…}` record
type) — are the same concept twice: pure slot-only markers carrying no data, distinguished
only by which part-kind they admit. Neither is ever a runtime value's `ktype()`, neither is
name-resolvable, and both out-specify `OfKind(Proper)` identically (via the generic
guard at [`ktype_predicates.rs`](../../src/machine/model/types/ktype_predicates.rs)). The
duplication shows as paired arms across the codebase:

- `matches_part` lists them as two adjacent arms
  ([`ktype_predicates.rs:426-427`](../../src/machine/model/types/ktype_predicates.rs)).
- The lazy-candidate part-match in [`pick.rs:60-69`](../../src/machine/core/kfunction/pick.rs)
  carries two near-identical `(variant, matching_part) => … | (variant, _) => return None`
  blocks.
- `name`, `PartialEq`, and `Hash` each carry the two as adjacent unit entries
  (`ktype.rs:234-235`, `335-336`, `440-441`).
- The mutual-exclusion gate in
  [`resolve_dispatch.rs:421-430`](../../src/machine/execute/dispatch/resolve_dispatch.rs)
  spells out "a `:{…}` must not be admitted to a `:SigiledTypeExpr` slot, nor a `:(…)` to a
  `:RecordType` slot" as two `!matches!(…, KExpression | <the other one>)` clauses.

`KExpression` is *not* part of this duplication: it is value-bearing — a
[`KObject::KExpression`](../../src/machine/model/values/kobject.rs) reports `ktype() =
KExpression`, it is name-resolvable (`"KExpression"` in
[`ktype_resolution.rs`](../../src/machine/model/types/ktype_resolution.rs)), registered as a
named type in [`builtins.rs`](../../src/builtins.rs), and gated specially by the scheduler
([`submit.rs`](../../src/machine/execute/scheduler/submit.rs)) and the lazy-candidate relax
in `resolve_dispatch.rs`. Folding it in would make the merged variant double as a value type
in one sub-case only, breaking the "slot-only marker" invariant.

**Acceptance criteria.**

- `KType` has one `RawTypePart(TypePartKind)` variant in place of `SigiledTypeExpr` and
  `RecordType`, where `TypePartKind` distinguishes the `:(…)` and `:{…}` part shapes;
  `KExpression` is unchanged.
- `matches_part` matches the merged variant in one arm that admits the part shape its
  `TypePartKind` names.
- The two `pick.rs` part-match blocks for the merged variant collapse to one keyed on
  `TypePartKind`.
- The `resolve_dispatch.rs` mutual-exclusion gate expresses "each raw-type-part slot admits
  only its own part shape" as a comparison between two `RawTypePart` kinds rather than two
  `KExpression | <other-variant>` clauses.
- `name`, `PartialEq`, and `Hash` each carry one arm for the merged variant.
- Behavior is unchanged: a `:{…}` part is still rejected by a `:(…)`-shaped raw-type-part
  slot and vice versa, and both still out-specify `OfKind(Proper)` so a raw-type-part
  overload still wins over a generic type slot when both admit.
- `KExpression`'s value-bearing roles (`KObject::KExpression` `ktype`, name resolution,
  scheduler gate, lazy-candidate relaxation) are untouched.

**Directions.**

- *Merge the pair only, not the trio — decided.* `SigiledTypeExpr` + `RecordType` are
  slot-only markers; `KExpression` is a value-bearing type that merely also works as a lazy
  slot, so it stays a distinct variant.
- *Variant shape `RawTypePart(TypePartKind)` with a two-case sub-enum — open.* A sub-enum
  `TypePartKind { Sigiled, Record }` keeps the part-shape distinction in one place and lets
  `matches_part` / `pick.rs` / the mutual-exclusion gate key on it. Alternative: two boolean
  helper predicates over a single marker. Recommended: the `TypePartKind` sub-enum.
- *Surface-keyword rendering — open.* `name()` currently renders `SigiledTypeExpr` /
  `RecordType` as those literal keywords; decide whether the merged variant renders a single
  `RawTypePart` keyword or dispatches the keyword on `TypePartKind`. Recommended: keep the
  two distinct surface keywords (render off `TypePartKind`) so existing surface forms and
  any round-trip stay stable.

## Dependencies

An engine-internal `KType` hygiene item on the type-language substrate; update
[design/typing/ktype.md](../../design/typing/ktype.md) if the variant vocabulary it names
changes.

**Requires:** none — engine-internal.

**Unblocks:** none tracked yet.
