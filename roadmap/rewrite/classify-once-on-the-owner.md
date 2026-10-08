# Classify once, on the type that owns the fact

**Problem.** Facts about a node's kind are re-derived at each reader as a
pattern match instead of being answered by the type that owns them. The
name-part set `Identifier | Type | MarkedName` is spelled five times across
[`evaluate.rs`](../../src/dispatch/evaluate.rs) and
[`statics.rs`](../../src/dispatch/statics.rs), so a name kind missed in
`instance_site` or `head` silently stops being an instance site. The
callable-kind set `{Lambda, Operator, UnaryOperator}` is spelled three times in
[the shape builder](../../src/scope/shape/build.rs) and `role.rs`, which says
`reading()` is the one authority though the rewrite never calls it. "A bare
`Identifier | Type` under `Role::Field` is a label" is spelled four times. "Is
a signature type" is the same three-variant `matches!` in `evaluate.rs` and
`statics.rs` though `KKind::Signature` exists. The
`DeclaredType::Type => node / Scheme => scheme_node` match recurs four times.
"Lands at the program's top level" is spelled three ways in the builder.
[`Which`](../../src/scope/shape.rs) is derived three ways, from the bucket
index and count in the builder and the surface, and from whether a bridge
exists in [`declaration.rs`](../../src/elaborate/declaration.rs), joined by
equality where a disagreement is a silent `None`. A member read is re-derived
in three `Form::Call` arms rather than classified once in `of_node`, and
`Static::Unknown` doubles as "read at a call's head". The per-role tables
(`walk_parts`, `walk_definition_roles`, `rewrite_node`, `code_marks`) are kept
in step by hand and already differ: `visit_node` declares a union's family
parameters, `walk_definition_statement` does not.

**Acceptance criteria.**

- `Role` and `BodyKind` answer `is_callable` and `label_reads`, and every site
  that classifies a callable kind or a label calls them; no `matches!` over
  those variant sets exists outside the owner.
- `Form` has a name variant and a member-read variant that `of_node` classifies
  once; `instance_site`, `head` and the `Form::Call` arms match the variant
  rather than the part kinds.
- `Which::of(count, index)` is the one derivation; `declaration.rs` reads it
  rather than a bridge's existence, and a disagreement cannot arise.
- `DeclaredType::node()` replaces the four matches; `KKind::Signature`
  replaces the two `matches!`.
- One top-level predicate replaces the three spellings in the builder.
- The parameter-declare walk is one function the statement walk and the
  definition walk share, pinned by a test that a union's family parameters are
  declared on both paths.
- The rewrite branches on `reading()`.

**Directions.**

- *Where a predicate lives — decided.* On the type that owns the fact (`Role`,
  `BodyKind`, `Form`, `DeclaredType`, `KKind`), never in a reader.
- *`Static::Unknown` as a flag — open.* A distinct `Static` variant for a
  head read, or the head read carried on `Form`. Recommended: on `Form`, since
  `of_node` already sees the call.
- *The per-role tables — open.* One table of `(Role, Reading, declares)` the
  four walks read, or four walks over shared predicates. Recommended: shared
  predicates first; a table only if the walks still drift.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
