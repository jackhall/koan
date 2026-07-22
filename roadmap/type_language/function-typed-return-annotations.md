# Bare parenthesized return annotations

Make the bare `-> (LIST OF Str)` / `-> (FN …)` return annotation behave like its
sigiled twin, which fails at definition today.

**Problem.** The two ways to annotate a constructed return type diverge. The
**sigiled** form `-> :(FN (x :Number) -> Number)` elaborates and runs — a closure
factory can declare and return a typed function (see the closure example in
[tutorial/04-functions.md](../../tutorial/04-functions.md)). The **bare** form,
parenthesized without the `:` sigil, fails at definition time: `FN`'s keyworded
overloads admit the return slot only as a bare type token (`-> Number`), a
sigiled type expression (`-> :(…)`), or an identifier (a diagnose-only overload)
— see the signatures in [`fn_def.rs`](../../src/builtins/fn_def.rs). A bare
`(LIST OF Str)` is a `KExpression` part, so no overload matches, resolution
defers, and the submit path stages *every* parenthesized part as an ordinary
sub-dispatch (`install_eager_only` in
[`keyworded.rs`](../../src/machine/execute/dispatch/keyworded.rs)). The
parameter list — meaningful only as a binder pattern — is then evaluated as a
call, and the definition dies with
`dispatch failed for SINGLETON s Str: no matching function`.

The gap is constructor-independent: `-> (FN (y :Number) -> Number)`,
`-> (LIST OF Str)`, and `-> (MAP Str -> Number)` all fail identically, while each
sigiled counterpart runs. The bare form parallels how every other return type is
written (`-> Number`, not `-> :Number`), so it is the natural thing to reach for
and a likely papercut. `OP`'s operand and return slots ride the same carrier
seam (`extract_type_slot_raw` in
[`return_type.rs`](../../src/builtins/fn_def/return_type.rs)) and exhibit the
same fall-through: `OP #(++) OVER (LIST OF Str) = (…)` stages its body eagerly
and fails with `unbound name 'left'`.

**Acceptance criteria.**

- A function declaring a bare parenthesized return type — `-> (LIST OF Str)`,
  `-> (MAP Str -> Number)`, `-> (FN :{…} -> …)` — elaborates and runs, at
  parity with the sigiled form.
- The body's returned value is checked against the declared type; a return that
  doesn't match surfaces a `TypeMismatch` for `<return>`, like every other
  return-type violation.
- A closure factory written with the bare form — `FN (ADDER n :Number) -> (FN :{x :Number} -> Number) = (…)`
  — type-checks its returned closure and is callable.
- `OP`'s operand and return slots accept the bare parenthesized form at parity
  with their sigiled counterparts — `OP #(++) OVER (LIST OF Str) = (…)`
  registers and dispatches.
- An anonymous record-schema function with a constructed return type —
  `FN :{s :Str} -> :(LIST OF Str) = (…)`, and its bare-parenthesized twin —
  elaborates, runs, and return-checks like the keyworded form (today this
  signature × return carrier combination falls through every `FN` overload).

**Directions.**

- *Where the bare form is admitted — decided.* At parse: binder discovery is
  parse-static, so the parser wraps a plain parenthesized part in a binder
  form's type slot as `ExpressionPart::SigiledTypeExpr` (the same
  `Box<KExpression>` payload), making `(…)` ≡ `:(…)` in exactly those slots.
  The overload table, the derived `chain_slot_mask`, and dispatch stay
  untouched, and parity with the sigiled form is exact by construction. Needs a
  per-spec type-slot mask beside `chain_slot_mask` in
  [`binder.rs`](../../src/machine/model/binder.rs)'s `BINDER_SPECS`. (The
  dispatch-side alternative — a `KEXPRESSION` type-slot overload — was
  rejected: it flips the bucket's derived mask entry under the
  `!= KEXPRESSION` AND-over-binder-overloads rule and grows the overload
  matrix per carrier combination.)
- *How the overload-matrix holes are closed — decided.* Collapse the carrier
  dimension with union-typed slots, shipped by the prerequisite
  [Union-typed carrier slots for builtin signatures](union-carrier-slots.md)
  item. On top of that machinery, this item rewrites
  [`fn_def.rs`](../../src/builtins/fn_def.rs) to two overloads — the
  binder-shaped keyworded form and the non-binder record form, each with a
  return slot of `union_of(of_kind(ProperType), SIGILED_TYPE_EXPR,
  IDENTIFIER)` and one body branching on the carrier
  (`extract_type_slot_raw` already branches on all three; the diagnose-only
  `IDENTIFIER` overload folds into an error arm) — and replaces
  [`op_def.rs`](../../src/builtins/op_def.rs)'s `for operand { for result { …
  } }` registration loop with flat union-slot registrations. The signature
  dimension stays two overloads because the binder flag is per-overload and
  genuinely differs. (Enumerating the missing matrix cells as overloads was
  rejected: the matrix re-leaks with every new carrier and the buckets keep
  growing.)
- *`OP`'s operand and return slots — decided.* The fix covers them in this
  item; they share the carrier seam and the overload-set gap with `FN`'s return
  slot.

## Dependencies

**Requires:**

- [Record-typed parameter list in the FN type constructor](fn-type-record-parameter-list.md)
  — the acceptance criteria write function types in the record form.
- [Union-typed carrier slots for builtin signatures](union-carrier-slots.md)
  — the overload-matrix collapse registers union-typed return and operand
  slots.

**Unblocks:** none.
