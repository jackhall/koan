# SIG abstract vs manifest type members

Distinguish abstract (open, shareable) from manifest (fixed) type members in module
signatures, matching OCaml's module-type model.

**Problem.** A SIG type member is declared one way — `LET Type = Number` inside the SIG body
([`src/builtins/sig_def.rs`](../../src/builtins/sig_def.rs)) — and that single form is a
*hybrid*: it carries a concrete witness (`Number`) yet also takes on an `AbstractType` identity
([`KType::AbstractType`](../../src/machine/model/types/ktype.rs)) when read through an opaque
ascription (`:|`, [`src/builtins/ascribe.rs`](../../src/builtins/ascribe.rs)) — e.g. `IntOrd.Type`
re-tags to the abstract `Type`, not `Number`. So koan cannot express the OCaml distinction:

- an **abstract** member (`type t`) — no witness, open for a client to share with any concrete
  type via a `WITH` sharing constraint;
- a **manifest** member (`type t = int`) — fixed, where a satisfying module's `t` *must* be
  `int` and a `with type t = <incompatible>` is rejected.

The `= Number` acts as a default-cum-witness rather than a fixity, so the two roles collapse into
one ambiguous form. The consequence shows at the `WITH` boundary: `Set WITH {Ord = Str}` re-pins a
slot declared `LET Ord = Number` with no error
([`with_two_slots_preserve_order`](../../src/builtins/type_ops/with.rs)), where OCaml would reject
re-pinning a manifest and impose no constraint on a truly abstract slot. The module-vs-SIG
satisfaction check ([`compatible_sigs`](../../src/machine/model/types/ktype.rs)) likewise has no
manifest-equality rule to enforce.

**Impact.**

- *SIGs express abstract vs manifest type members*, so a signature states which type slots are
  open for sharing and which are fixed — the OCaml module-type vocabulary.
- *Module satisfaction is sound on type members*: a module matches a SIG only when its manifest
  type members equal the declared ones, while abstract members stay free.
- *`WITH` sharing constraints carry their real meaning*: pinning targets abstract slots, and an
  incompatible manifest pin is a type error rather than a silently-accepted re-binding.
- *The `AbstractType` identity rests on a clean split* rather than the witness-with-default hybrid,
  so opaque ascription and abstract-type identity have one coherent model to reason about.

**Directions.**

- *Abstract-member surface — open.* koan has only the witness-bearing `LET Type = Number` form;
  an abstract (witness-less) type slot needs its own surface. Candidates: a dedicated `TYPE Elt`
  declarator, or a distinguished witness-less slot form. Recommended: a `TYPE` declarator so
  abstract and manifest read distinctly at the SIG.
- *Manifest fixity — open.* A manifest `LET Type = Number` fixes `Type`; decide whether `WITH` on a
  manifest is an error always, or a no-op when the pin equals the manifest (OCaml allows the
  redundant equal pin). Recommended: allow an equal pin, reject an incompatible one.
- *Satisfaction rule — decided.* A module matches a SIG iff every manifest type member equals the
  module's corresponding type and every abstract member is unconstrained — the OCaml rule, threaded
  through `compatible_sigs`.
- *`WITH` targets abstract slots — decided.* `WITH` pins only abstract members; the current free
  re-pin tightens (the `Ord = Str` onto a `Number` slot case becomes a manifest-fixity error).
- *Untangle the `AbstractType` hybrid — open.* The opaque-ascription re-tag (`:|` / `:!`, the
  `IntOrd.Type` → `AbstractType` path) rests on the witness-with-default model; rework so abstract
  members carry no witness and manifest members carry no abstract identity. This is the
  load-bearing, intricate part. Recommended: spike the representation change against the
  opaque-ascription tests first.

## Dependencies

**Requires:** none — builds on the shipped SIG / opaque-ascription / `AbstractType` substrate.

**Unblocks:** none tracked yet.

Independent of the `SIG_WITH` → infix `WITH` surface migration (the `WITH` builtin's slot
validation is unconstrained today precisely because there is no manifest/abstract distinction to
enforce — this item is what would give it teeth).
