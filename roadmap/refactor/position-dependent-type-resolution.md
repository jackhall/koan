# Retire the lexical-visibility carve-outs

Make type-name resolution obey source order like the value language, leaving one
visibility rule and no per-binding or per-callsite bypass.

**Problem.** Lexical visibility is one predicate —
`Bindings::visible(idx, cutoff)` = `nominal_binder || idx < cutoff`
(`src/machine/core/bindings.rs:312`). The value language honours it strictly: FN and
LET install value-gated — the trailing `false` is the nominal-binder flag, `FN` is
value-side gated (`fn_def.rs:224,244`), and the placeholder index is
`{idx: chain.index, nominal_binder: p.is_nominal_binder}`
(`scheduler/submit.rs:229-232`), so the flag is the only lever. The type language does
not. Two bypass kinds remain:

- *Per-binding.* STRUCT / UNION / SIG / MODULE / FUNCTOR install
  `nominal_binder: true` (`struct_def.rs:73`, `union.rs:71`, `sig_def.rs:50`,
  `module_def.rs:53`, `functor_def.rs:98`), so they resolve regardless of source order.
  The actual engine of mutual type recursion, though, is the *chainless elaborator*:
  `Elaborator::new` takes only a scope, no `LexicalFrame` (`resolver.rs:85`), and
  `resolve` / `resolve_type` hard-wire `cutoff = None` (`scope.rs:457,515`). So
  `elaborate_type_expr`'s `Resolution::Placeholder` arm (`resolver.rs:123,133`) sees the
  forward placeholder and `close_type_cycle` (`resolver.rs:223`) closes the SCC — none
  of which reads `nominal_binder`. **Proof the flag is droppable:** the recursion path
  passes `None`, so `visible` short-circuits to `true` before the flag is consulted;
  removing `nominal_binder` from the five binders leaves that path byte-for-byte
  unchanged. The flag's only live effect is the value-dispatch-side complement that lets
  a value site name a forward-declared type. The `struct_def/tests/recursion.rs` and
  `functor_def/tests/recursive_carrier.rs` fixtures exercise the elaborator path and are
  the falsification target.
- *Incidental.* FN parameters and MATCH / TRY `it` install `nominal_binder: true` at
  `idx 0` (`invoke.rs:94`, `match_case.rs:84`, `try_with.rs:117`) only to clear the
  single-statement body cutoff: `assemble_body_chain` passes `body_index = 0` for a
  lone body statement (`lexical_frame.rs:91,110`), so `visible(idx 0, Some(0))` is
  `0 < 0` = false (`bindings.rs:312`) and the param/`it` needs the flag to be seen. An
  indexing workaround, not a forward-reference policy.

Several other `cutoff = None` reads are not lexical-sibling questions at all — module
and signature member access (`attr.rs:103,183,241`), USING collision checks
(`scope.rs:275`), binder self-reads at finalize (`struct_def.rs:114`,
`union.rs:109`, `module_def.rs:63`), the SCC finalize-gate probe (`resolver.rs:253`,
`resolve_type_expr.rs:117`), and the pre-dispatch binder scan (`submit.rs:38`). They
share the same `None`-defaulting API as the genuine carve-outs, which is what lets the
type bypass read as an ordinary unfiltered lookup. Each already documents why it is
unfiltered at its callsite (`submit.rs:35-36`, `resolve_type_expr.rs:112-115`), so
Phase 2 is a mechanical re-home, not a fresh judgment.

**Impact.**

- *One visibility rule across both languages.* Type names obey source order exactly as
  FN / LET do; a forward type reference at a value site is a position error, not a
  silent success.
- *Mutual type recursion rides deferred `RecursiveRef`.* The self-reference mechanism
  generalizes to all forward references, so recursion closes without forward
  placeholder visibility.
- *The unfiltered-read API carries one meaning.* `cutoff = None` means "no cutoff"
  only; the closed-unit and self reads move behind a distinct method, so every
  remaining bypass is auditable at its callsite.
- *The bare-leaf fold lands once.* Folding `coerce_type_token_value` into a
  chain-gated `resolve_type_expr` reaches correct semantics in a single pass instead of
  baking in the carve-out and redoing it.

**Directions.**

- *Phase 1 — drop FN-param / `it` nominal via an indexing fix — decided.* Index
  parameters and `it` in a tier genuinely below statement 1 (or give a single-statement
  body cutoff ≥ 1) so they resolve via `idx < c`, then flip them to value-gated. No
  forward-reference policy is involved.
- *Phase 2 — segregate the non-lexical `None` reads — decided.* Give module / signature
  member access, binder self-reads, the SCC finalize-gate probe, and the pre-dispatch
  binder scan a distinct method (e.g. `lookup_member` / `lookup_own`), so
  `resolve_type(None)` stops meaning two things and the genuine carve-outs are the only
  `None` callers left.
- *Phase 3 — chain-gate the elaborator — mostly shipped.* `Elaborator` carries a
  `LexicalFrame` and resolves bare leaves via `resolve_type_with_chain` /
  `resolve_with_chain`; `Scope::resolve_type_expr` takes a chain and the `type_expr_memo`
  re-keys by `(TypeName, cutoff)`. The five binders (STRUCT / UNION / SIG / MODULE /
  FUNCTOR) install non-nominal, so a forward type reference is a position error and the
  reactive SCC seal (`detect_pending_cycle` / `seal_type_cycle` / pending-edge bookkeeping)
  is retired; mutual recursion is expressed with a `RECURSIVE TYPES` block. The binder
  field sites (STRUCT/UNION/NEWTYPE), FN parameters, and FUNCTOR parameters are gated.
  *Remaining:* the return-type-position resolvers (`fn_def/return_type.rs`,
  `branch_walk.rs`) and the sigil sub-dispatch finish closures (`type_constructors.rs`)
  still pass `chain = None`, so a forward type reference *in a return type or a
  `:(LIST OF Later)` sigil* is not yet a position error — thread the captured chain through
  those sites. The `nominal_binder` field / `is_nominal_binder` / `register_nominal_binder`
  are now dead and collapse to `idx < c` in Phase 4.
- *`BindingIndex::BUILTIN` stays — decided (out of scope).* `idx 0` / non-nominal is the
  base "declared before statement 1," correctly visible via `0 < c`; it is not a
  carve-out.

## Dependencies

**Requires:**

- [Lookup protocol](../../design/typing/lookup-protocol.md) — the per-scope
  `visible(idx, cutoff)` walk and `LexicalFrame` chain this work re-points the type
  language onto.
- [`RECURSIVE TYPES` block](../type_language/recursive-types-block.md) — the explicit
  co-declaration expressing mutual recursion once forward type references are position
  errors; without it, chain-gating the elaborator would break mutually-recursive types.

**Unblocks:**

- [Collapse the bare-leaf type resolvers](collapse-bare-leaf-type-resolvers.md) — once
  the elaborator is chain-gated, folding `coerce_type_token_value` into
  `resolve_type_expr` lands on matching visibility semantics instead of dropping the
  cutoff.
