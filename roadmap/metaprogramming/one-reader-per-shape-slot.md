# One kind-blind reader per shape slot

**Problem.** The declaration surfaces read their shape slots — `OP`'s symbol,
`FN`'s signature, `GROUP`'s pairwise combiner — through two extractors that
disagree on part kind, and each surface half-accepts the spelling it doesn't
document. The slot-side read takes the shape from the evaluated `:KExpression`
argument, where a `QuotedExpression` part and a plain `Expression` part are
indistinguishable — both resolve to a `KObject::KExpression` — so both
spellings pass and the declaration's bodies and registry entries land. The
statement-side reads scan the *unevaluated* parts and each match one kind
only:

- [`op_def::symbol_from_parts`](../../src/builtins/op_def.rs) matches
  `QuotedExpression` only, so an unquoted `OP (+) OVER Number = (…)` silently
  installs no pending-overload park edges (the binder hook discards the
  diagnostic) and hard-errors inside a `GROUP` body (the member scan
  propagates it);
- [`fn_def::signature::signature_expr_part`](../../src/builtins/fn_def/signature.rs)
  matches `Expression` only, so a quoted `FN #(ADD left :Number right :Number)
  = (…)` registers its overload but installs no park edges.

Either way the declaration reaches a partial state — bodies registered, park
edges missing — whose visibility to same-block siblings depends on scheduler
order. [design/metaprogramming.md](../../design/metaprogramming.md) specifies
the rule this work enforces: the two literal spellings are one spelling, each
shape slot has exactly one kind-blind reader shared by every consumer, and a
declaration either fully registers or is fully refused.

**Acceptance criteria.**

- `OP (+) OVER Number = (…)` and `OP #(+) OVER Number = (…)` behave
  identically in every context — bare, inside a `GROUP` body, and under
  `UNARY`: both register the same overloads and registry entry, both install
  the same park edges, both join the same group memberships.
- `FN (ADD left :Number right :Number) = (…)` and its quoted spelling behave
  identically likewise, park edges included.
- `GROUP … PAIRWISE FOLD (BOTH) LEFT` and the quoted combiner spelling behave
  identically.
- Each shape slot has one reader shared by the dispatch-time slot read, the
  binder hook, and `GROUP`'s member scan; all consumers reach the same verdict
  on the same declaration, and no consumer discards a diagnostic another
  surfaces.
- No declaration can register bodies or a registry entry while failing to
  install its park edges.
- A dynamic expression argument in a shape slot — an identifier or any
  non-literal part, e.g. `FN sig = (…)` with `sig` bound to an assembled
  signature — is refused at the declaration with a diagnostic and registers
  nothing.

**Directions.**

- *Where the dynamic-part rejection lands — decided.* Statement-side, in the
  shared reader: part-kind is a parse-static fact, and the binder hook and the
  declaration body must reach the same verdict, so the check lives in the one
  reader both call. The hook may not discard a diagnostic.
- *Diagnostic wording — decided.* The rejection names the assembly route:
  build the full declaration as an expression value and splice it with `EVAL`
  ([design/metaprogramming.md](../../design/metaprogramming.md)).

## Dependencies

The rejection diagnostic's suggested route ships with
[EVAL splices in place](eval-splices-in-place.md); nothing here blocks on it —
the reader unification and rejection are correct on their own.

**Requires:** none — fixes a defect in the shipped declaration surfaces.

**Unblocks:** none tracked.
