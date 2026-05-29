# Name the five elaboration layers in `design/typing/elaboration.md`

Add a "Layers" section to `design/typing/elaboration.md` (already the
protocol's primary home) that names Layer 1–5 of the TypeExpr
elaboration pipeline and lists each layer's source-file home. The
four other docs that currently each restate their slice of the
pipeline cross-link the section instead.

**Problem.** The full pipeline from parser `TypeExpr` to fully-
elaborated `&'a KType` has five layers, each in a different file:

1. `OnceCell<KType>` on `TypeExpr` itself — scope-independent builtin
   lowering ([`ast.rs`](../../src/machine/model/ast.rs)).
2. `Bindings.type_expr_memo` — scope-bound memo
   ([`bindings.rs`](../../src/machine/core/bindings.rs)).
3. The elaborator proper — recursion via threaded set, finalize gate
   ([`resolver.rs`](../../src/machine/model/types/resolver.rs)).
4. `coerce_type_token_value` — bare-leaf dispatch ingress
   ([`resolve_type_expr.rs`](../../src/machine/core/resolve_type_expr.rs)).
5. `KObject::TypeNameRef` — surface-form-survives-bind carrier for
   unresolvable-at-bind-time names
   ([`kobject.rs`](../../src/machine/model/values/kobject.rs)).

The pipeline is the densest "how does this actually work" sprawl in
the type-system docs, described across
[`elaboration.md`](../../design/typing/elaboration.md) (Layer 1 /
Layer 2 / threaded set / bare-leaf carrier — primary home),
[`ktype.md`](../../design/typing/ktype.md) (`TypeExprRef` slot kind,
`TypeNameRef` carrier, `from_type_expr` arity check),
[`functors.md`](../../design/typing/functors.md) (`Deferred(_)`
carrier through `Combine` finish + per-call elaboration),
[`type-language-via-dispatch.md`](../../design/typing/type-language-via-dispatch.md)
(`:(...)` sigil feeding the elaborator through dispatch), and
[`execution-model.md`](../../design/execution-model.md) (FN-signature
parking on `ElabResult::Park`, the wrap-slot splice walk's coerce
call). Each doc paraphrases the layer it touches without referencing
the broader sequence.

The code is correctly distributed for real structural reasons:
`KObject::TypeNameRef` can't move (enum variant),
`type_expr_memo` lives on `Bindings` for scope-bound caching, the
elaborator's threaded set ties it to `resolver.rs`. The candidates
analysis rejected the code-level consolidation (Pass 15 Δ +1.56 even
with doc consolidation). The remaining work is the doc-only one: a
single section that names the layers and lets each doc cross-link
its concern instead of paraphrasing the surrounding sequence.

**Impact.**

- One section in `elaboration.md` names Layer 1–5 and gives each a
  source-file home. The 5-layer mental model becomes a single read.
- Four other typing docs (`ktype.md`, `functors.md`,
  `type-language-via-dispatch.md`, `execution-model.md`) each
  shorten their per-layer paraphrase to a cross-link of the form
  `see [design/typing/elaboration.md § Layers] § Layer N`.
- The densest "how does this work" sprawl in the type-system docs
  is addressed without the code refactor the metric rejected.
- New contributors investigating type elaboration land on one
  section that names the pipeline shape, then follow specific
  layers into the docs that own their consequences.

**Directions.**

- **Doc-only seam — decided per Pass 15.** The code-level seam was
  rejected (Δ +2.41 alone, +1.56 with consolidation) because
  `KObject::TypeNameRef` is an enum variant that can't move and
  `type_expr_memo` has structural reasons to live on `Bindings`.
- **Section lives in `elaboration.md`, not a new doc — decided.**
  `elaboration.md` is already the pipeline's primary home; pulling
  the layer enumeration into a separate doc would orphan the
  existing layer 1 / 2 / threaded-set content. Section header:
  `## Layers`, near the top after the orienting paragraphs.
- **Layer numbering — decided.** Layers 1–5 as enumerated above.
  The numbering matches the candidates analysis (#8); changing it
  would force unnecessary churn across cross-links.
- **Inbound link rewrites — decided.** Each of the four other docs
  keeps its topic-specific content; their per-layer paraphrases
  trim to a `see § Layer N` cross-link.

## Dependencies

**Requires:** none.

**Unblocks:** none.
