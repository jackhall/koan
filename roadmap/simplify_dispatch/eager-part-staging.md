# One owner for eager-part staging

Give the eager-part shape taxonomy a single constructor-classifier and stage
directly in `DepRequest` currency.

**Problem.** The six eager part shapes (`Expression` / `SigiledTypeExpr` /
`RecordType` / `ListLiteral` / `DictLiteral` / `RecordLiteral`) are enumerated
four times across the dispatch subsystem: the full "stage as a pending sub +
push an empty-Identifier placeholder" match is written out in
`stage_all_eager_parts` ([dispatch.rs](../../src/machine/execute/dispatch.rs))
and again in the eager-filter tail of `part_walk`
([keyworded.rs](../../src/machine/execute/dispatch/keyworded.rs)), the same
shapes are re-enumerated as `is_eager_part`
([resolve_dispatch.rs](../../src/machine/execute/dispatch/resolve_dispatch.rs)),
and `classify_aggregate_part`
([literal.rs](../../src/machine/execute/dispatch/literal.rs)) classifies the
same set with a different output. Nothing ties the four together — adding a
new eager shape means finding and extending each by hand.

The staging currency doubles the problem: `PendingSub`
([dispatch.rs](../../src/machine/execute/dispatch.rs)) is a private
near-isomorphic copy of `DepRequest`
([action.rs](../../src/machine/core/kfunction/action.rs)) whose sole consumer,
`SchedulerView::install_eager_subs`
([ctx.rs](../../src/machine/execute/dispatch/ctx.rs)), is a 1:1
variant-rename loop — a type that exists only to be converted into another
type.

**Acceptance criteria.**

- A single constructor-classifier owns the six-arm eager-shape match;
  `stage_all_eager_parts`, `part_walk`'s eager-filter tail, `is_eager_part`,
  and `classify_aggregate_part`'s eager arms all derive from it, and the
  eager-shape enumeration appears in exactly one dispatch-side match.
- `PendingSub` is deleted: the part walk stages eager parts directly in
  `DepRequest` currency, and `install_eager_subs` performs no
  variant-to-variant rename.
- Dispatch behavior is unchanged — the same subs install at the same slot
  indices with the same placeholders — with existing tests green.

**Directions.**

- *Classifier shape — open.* (a) `fn stage_eager_part(&ExpressionPart) ->
  Option<DepRequest>`, with `is_eager_part` becoming `.is_some()` (or deleted
  in favor of direct calls); (b) a `PartKind` classification enum consumed by
  each site. Recommended: (a) — the sites want the staged value, not a label,
  and `classify_aggregate_part`'s divergent output can wrap the same
  constructor.
- *`classify_aggregate_part` delegation depth — open.* Its aggregate output
  differs from plain staging; decide whether its eager arms call the shared
  constructor or only share the shape predicate.
- *Empty-Identifier placeholder sentinel — deferred.* The
  `ExpressionPart::Identifier(String::new())` hole marker that staging pushes
  is a stringly convention; typing it as a staged-slot representation is a
  follow-up once this item settles what a staged slot is, and is tracked only
  here until it earns its own item.

## Dependencies

Touches the same `part_walk` / staging seams as
[binder-discovery-to-parse.md](binder-discovery-to-parse.md); coordinate if
both are in flight.

**Requires:** none — self-contained dispatch cleanup.

**Unblocks:** none tracked yet.
