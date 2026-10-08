# Classify once, on the type that owns the fact

**Problem.** Facts about a node's kind are re-derived at each reader as a
pattern match instead of being answered by the type that owns them. The
name-part set `Identifier | Type | MarkedName` is spelled six times across
[`evaluate.rs`](../../src/dispatch/evaluate.rs) and
[`statics.rs`](../../src/dispatch/statics.rs), so a name kind missed in
`instance_site` or `head` silently stops being an instance site; `head` also
looks a name up at the site of the parentheses around it, so
`((pick) {x = 2})` is refused at load although the builder admits it and the
run reads it. The callable-kind set `{Lambda, Operator, UnaryOperator}` is
spelled three times in [the shape builder](../../src/scope/shape/build.rs) and
`role.rs`, which says `reading()` is the one authority though the rewrite never
calls it. "A bare `Identifier | Type` under `Role::Field` is a label" is
spelled four times. `view::is_signature_type` spells the three signature nodes
although `KType::kind_of` already answers `KKind::Signature` for exactly those.
The `DeclaredType::Type => node / Scheme => scheme_node` match recurs eight
times. "Lands at the program's top level" is spelled three ways in the builder.
[`Which`](../../src/scope/shape.rs) is derived three ways, from the bucket
index and count in the builder and the surface, and from whether a bridge
exists in [`declaration.rs`](../../src/elaborate/declaration.rs), joined by
equality where a disagreement is a silent `None`. A member read is re-derived
in three `Form::Call` arms rather than classified once in `of_node`, and
`Static::Unknown` in the instance table doubles as "read at a call's head",
which one reader treats as unreachable and another relies on. The per-role
walks are kept in step by hand and already differ: `visit_node` declares a
union's family parameters, `walk_definition_statement` does not, so a
parameterized `UNION` in a `SIG` body reports its own parameter unbound.

**Acceptance criteria.**

- `Role` and `BodyKind` answer `is_callable` and `label_reads`, and every site
  that classifies a callable kind or a label calls them; no `matches!` over
  those variant sets exists outside the owner.
- `Form` has a name variant and a member-read variant that `of_part` and
  `of_node` classify once; `instance_site`, `peeked`, `head` and the load's
  `form` match the variants rather than the part kinds.
- A quantified function named at a call's head through parentheses runs:
  `((pick) {x = 2})` prints `2`.
- `Which::of(count, index)` is the one derivation; `declaration.rs` reads it
  rather than a bridge's existence, and the join that reads it fails loudly
  rather than returning `None`.
- `DeclaredType::node()` replaces the eight matches; `KType::kind_of` replaces
  `view::is_signature_type`.
- One top-level predicate replaces the three spellings in the builder.
- The parameter-declare walk is one function the statement walk and the
  definition walk share, pinned by a test that a union's family parameters are
  declared on both paths.
- The load's instance table names a head read with a variant of its own, and no
  `Static::Unknown` is recorded there.

**Directions.**

- *Where a predicate lives — decided.* On the type that owns the fact (`Role`,
  `BodyKind`, `ExpressionPart`, `Form`, `Which`, `DeclaredType`, `KType`),
  never in a reader.
- *The head-read flag — decided.* A dedicated instance-table entry,
  `InstanceRead { Solved(StaticSolution), AsIs }`. It cannot ride on `Form`:
  the run reads it in the `ATTR` native, a child evaluation that never sees the
  application's `Form`.
- *The per-role walks — decided.* Shared predicates (`label_reads`,
  `is_callable`, one parameter-declare function), no `(Role, Reading, declares)`
  table. The rewrite keeps its exhaustive `Role` match: `reading()` alone cannot
  drive it, since `Bare` and `Container` each split into parts it rewrites and
  parts it leaves.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
