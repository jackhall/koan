# Bare parenthesized return annotations

Make the bare `-> (LIST OF Str)` / `-> (FN …)` return annotation behave like its
sigiled twin, which fails at definition today.

**Problem.** The two ways to annotate a constructed return type diverge. The
**sigiled** form `-> :(FN :{x :Number} -> Number)` elaborates and runs — a closure
factory can declare and return a typed function (see the closure example in
[tutorial/04-functions.md](../../tutorial/04-functions.md)). The **bare** form,
parenthesized without the `:` sigil, fails at definition time: the return slot's
carrier union in [`fn_def.rs`](../../src/builtins/fn_def.rs) admits only
raw-captured part kinds — a bare type token (`-> Number`), a sigiled type
expression (`-> :(…)`), a record type (`-> :{…}`), an identifier — and a bare
`(LIST OF Str)` is an ordinary `Expression` part with no raw-capture stamp, so
it evaluates eagerly to a resolved `ProperType` no union member admits and the
definition dies with
`dispatch failed for FN KExpression -> ProperType = KExpression: no matching function`.

The gap is constructor-independent: `-> (FN :{y :Number} -> Number)`,
`-> (LIST OF Str)`, and `-> (MAP Str -> Number)` all fail identically, while each
sigiled counterpart runs. The bare form parallels how every other return type is
written (`-> Number`, not `-> :Number`), so it is the natural thing to reach for
and a likely papercut. `OP`'s operand and return slots ride the same carrier
seam (`TypeSlotThunk::from_slot` in
[`return_type.rs`](../../src/builtins/fn_def/return_type.rs)) and fail the same
way: `OP #(++) OVER (LIST OF Str) = (…)` misses every `OP` overload.

Separately, the builtin registry carries eight diagnose-only registrations —
[`op_def.rs`](../../src/builtins/op_def.rs)'s two missing-result `UNARY OP … = …`
keys, and the type-named diagnostic overloads of
[`fn_def.rs`](../../src/builtins/fn_def.rs),
[`module_def.rs`](../../src/builtins/module_def.rs) and
[`group_def.rs`](../../src/builtins/group_def.rs), plus the diagnose-only
`IDENTIFIER` member of `FN`'s return union — whose only job is rendering
a targeted message on an inevitable miss. They clutter the success-path
registry, and the two missing-result keys would each need spec-table masking of
their own to keep their diagnostics at parity once the bare spelling is
admitted.

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
  elaborates, runs, and return-checks like the keyworded form (today the
  bare-parenthesized twin falls through every `FN` overload).
- The builtin registry contains no diagnose-only registrations or union
  members: the missing-result `UNARY OP … = …` forms (bare and `LET`-combined),
  the type-named `LET … = FN` / `MODULE` / `GROUP` shapes, and the value-named
  `FN` return slot (`-> er`, the identifier bound or unbound) surface their
  targeted messages from a dispatch-miss diagnosis table, with unchanged
  message texts — and the missing-result diagnostic fires for the
  bare-parenthesized operand spelling at parity with the sigiled one.
- A user overload registration under a reserved diagnostic key (`UNARY OP _
  OVER _ = _` or its `LET`-combined twin) is refused like any builtin-key
  shadow.

**Directions.**

- *Where the bare form is admitted — decided.* At parse: binder discovery is
  parse-static, so the parser wraps a plain parenthesized part in a binder
  form's type slot as `ExpressionPart::SigiledTypeExpr` (the same
  `Box<KExpression>` payload), making `(…)` ≡ `:(…)` in exactly those slots.
  The overload table and dispatch stay untouched, and parity with the sigiled
  form is exact by construction. Needs a per-spec type-slot mask in
  [`binder.rs`](../../src/machine/model/binder.rs)'s `BINDER_SPECS`. (The
  dispatch-side alternative — a `KEXPRESSION` type-slot overload — was
  rejected: it flips the slot's eager/lazy classification and grows the overload
  matrix per carrier combination.)
- *Where diagnose-only registrations live — decided.* A static dispatch-miss
  diagnosis table beside the sibling spec tables in `machine/model`, probed in
  the `Unmatched` arm of dispatch resolution
  ([`keyworded.rs`](../../src/machine/execute/decide/keyworded.rs)): each entry
  pairs a full untyped key spec with a render fn that confirms the mistake from
  the raw parts, else the generic miss reason stands. The missing-result keys
  unregister and become *reserved* — the overload shadow guard refuses user
  registration under a reserved key, so the shape stays unshadowable and the
  diagnosis stays sound — and their `LAZY_SLOT_SPECS` entries stay, so the
  statement reaches the miss with raw slots instead of dying on an eagerly
  evaluated body. The type-named diagnostic overloads migrate too, as does the
  diagnose-only `IDENTIFIER` member of `FN`'s return union; those keys keep
  success-path siblings. Because a value-named return whose identifier is
  unbound (a parameter name, the common case) surfaces as `UnboundName` rather
  than a dispatch miss, the table is probed from **both** terminal arms —
  `Unmatched` and `UnboundName` — before either generic rendering.
- *`OP`'s operand and return slots — decided.* The fix covers them in this
  item; they share the carrier seam and the overload-set gap with `FN`'s return
  slot.

## Dependencies

**Requires:** none — the
[union carrier slots](../../design/typing/ktype/slots-and-signatures.md#union-carrier-slots)
the return/operand slots ride are shipped.

**Unblocks:** none.
