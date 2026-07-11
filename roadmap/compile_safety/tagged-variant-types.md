# MATCH as ordinary type-dispatch

Retire `MATCH`'s parallel elimination walk — the residue of the tagged-union
work whose identity, dispatch, surface, and by-type arm selection already
[shipped](../../design/typing/user-types.md#unions-dissolve-into-per-variant-newtypes).

**Problem.** `MATCH` already selects arms by type: each head resolves to a
`KType`, admission runs
[`KType::matches_value`](../../src/machine/model/types/ktype_predicates.rs), and
the strictly most-specific admitting arm wins (ruling F1). But the selection
lives in a parallel walk —
[`find_branch_body_by_type`](../../src/builtins/branch_walk.rs) re-implements
the admission pass and the specificity tournament that ordinary dispatch
already owns
([`OverloadBucket::pick_strict`](../../src/machine/execute/dispatch/resolve_dispatch.rs)
over
[`ExpressionSignature::most_specific`](../../src/machine/model/types/signature.rs))
— so the language carries two elimination forms for typed values. The walk also
keeps its own value-bind site:
[`match_case.rs`](../../src/builtins/match_case.rs) deep-clones the scrutinee up
front and pairs it with caller-supplied reach evidence
(`branch_walk`'s `ItSource::Value`), where ordinary dispatch binds arguments
through its own bind leg. A typo'd arm head over a variant scrutinee errors
with only the unresolved name, without naming the scrutinee union's variants.
Separately, a schema field typed as a *sibling variant* of the union being
sealed is unsupported: the union-qualified sigil (`:(Tree Leaf)`) fails because
the union name is unbound while its schema is under seal — normal sigil
dispatch would park on the very producer awaiting this field — while union
self-reference (`Succ :Nat`) and variant sigils naming an already-sealed union
resolve fine.

**Acceptance criteria.**

- `MATCH`'s typed arm selection runs through the shared
  filter→`most_specific` tournament core that resolves ordinary overload
  buckets; `branch_walk` carries no duplicate admission or specificity logic.
- Boolean-literal heads (`true ->` / `false ->`) and `Result` tag heads
  (`Ok` / `Error`) select through an exact-match pre-pass ranked strictly above
  every typed arm; `MATCH` on `Ok` / `Error` over a `Result` keeps working as
  [documented](../../design/error-handling.md).
- The converged path carries no MATCH-specific value-bind site: the scrutinee
  (or, for a member-tag arm, its wrapped payload — ruling F3) reaches the
  selected arm through the ordinary dispatch bind leg, retiring
  `match_case.rs`'s scrutinee deep-clone paired with caller-supplied reach
  evidence.
- An arm head that resolves to no type over a union-variant scrutinee errors
  naming the scrutinee's variants — `` match arm type `Bogus` is not a known
  type; the scrutinee's union variants are `Some`, `None` `` — not just the
  unresolved name. (The variant list comes from the scrutinee's
  `RecursiveSet` members; the union's binder name is not recoverable from the
  value and is not part of the message.)
- A schema field can be typed as a sibling variant of the union currently
  being sealed via the union-qualified sigil (`Node :(Tree Leaf)`): the
  reference seals to its
  [`SetLocal`](../../src/machine/model/types/recursive_set.rs) form and
  resolves back at projection like any intra-set reference. A bare sibling tag
  (`Node :Leaf`) stays an unknown-type error — tags are never bare names, even
  in the declaring schema.

**Directions.**

- *Arm-selection semantics — decided, shipped.* Arms admit by
  `matches_value` and compete most-specific-wins (ruling F1); a member-tag head
  over a variant value admits by member `SetRef` identity and binds the wrapped
  payload to `it` (ruling F3); admitting arms with no strict winner are an
  ambiguity error. The remaining work changes the machinery, not these
  semantics. See
  [user-types.md § Unions dissolve into per-variant newtypes](../../design/typing/user-types.md#unions-dissolve-into-per-variant-newtypes).
- *Lowering mechanism — decided.* Shared tournament core: `MATCH` resolves its
  arm heads, then delegates admission and specificity to the same
  filter→`most_specific` core `OverloadBucket::pick_strict` uses, and delivers
  the arm's `it` value through the ordinary dispatch bind door. No
  per-execution function materialization — arms do not lower to ephemeral
  `KFunction` overloads (rejected: N materializations per `MATCH` execution,
  felt first in hot tail loops). `MATCH` keeps a thin adapter that walks the
  `<head> -> <body>` triples and chooses payload-vs-scrutinee for `it` (F3);
  ordinary FN parameter binding gains no unwrap-at-bind semantics.
- *Non-type arm heads — decided.* Exact pre-pass: boolean-literal and
  `Result`-tag heads are checked first as exact matches, which rank strictly
  above every typed arm (the ordering `ArmType::Exact` already encodes); only
  type-named heads enter the shared tournament. Restricting `MATCH` to typed
  heads was rejected — it breaks the documented `MATCH` on `Ok` / `Error`
  surface.
- *Sibling-variant reference spelling — decided.* Union-qualified sigil only:
  `Node :(Tree Leaf)`, uniform with the one variant spelling that exists
  everywhere else. The elaborator recognizes a sigil head naming the
  under-seal binder (the threaded name that already resolves self-references)
  and folds the `(ThreadedName Tag)` pair straight to the member's `SetLocal`
  — it must not park, since the producer it would park on is the seal awaiting
  this field. Bare member tags in the declaring schema were rejected: threading
  them would silently shadow an identically-named outer type, and tags stay
  unreachable as bare names everywhere.

## Dependencies

**Requires:**

- [Branch-arm return contract](../../design/execution/calls-and-values.md#arms-as-own-blocks)
  — the `MATCH` arm machinery the remaining work lowers into type-dispatch.

**Unblocks:**

- [Witness-derived binding](witness-derived-binding.md) — retiring
  `branch_walk`'s MATCH bind shrinks the bind-leg surface the fused door
  covers.
