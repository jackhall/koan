# Plain-English type-operation surfaces

Retire the `type_ops.rs` underscore-keyword family into the spaced, dotted, and
infix forms that already express the same operations, leaving no underscore
keyword and no redundant leading-keyword type operation.

**Problem.** Every type operation now reads as its plain-English surface — `:(LIST OF T)`,
`:(MAP K -> V)`, the dotted `M.T`, the infix `sig WITH {…}` — and no underscore
type-operation keyword remains. One redundant dispatch surface is left: the parens-form
computed return type. `-> (… WITH {…})` (and any `-> (expr)`) is an `Expression` part, so FN
and FUNCTOR each register a return-type overload whose slot is `KType::KExpression` purely to
admit it ([`src/builtins/fn_def.rs`](../../src/builtins/fn_def.rs),
[`src/builtins/functor_def.rs`](../../src/builtins/functor_def.rs)) — a second carrier
alongside the `TypeExprRef` (bare) and `SigiledTypeExpr` (`:(…)` / dotted) return overloads
that already admit every other return form. Its remaining clients are the still-parenthesized
computed `WITH` returns (`-> (Set WITH {Elt = Er.Type})`) and the bare-parens cases in
`fn_def/tests/body_routing.rs`.

**Impact.**

- *Type operations read as plain English.* `LIST_OF` / `DICT_OF` are spelled with
  the existing spaced forms (`:(LIST OF)`, `:(MAP ->)`). (A module type-member already
  reads as the dotted `M.T`, and signature specialization as the infix `sig WITH {…}`.)
- *One surface per type operation.* The `TypeExprRef` family folds into the
  `:(...)` Type forms and the `.` operator, so each operation has a single
  spelling and no underscore keyword token remains in the language.
- *FN and FUNCTOR drop the parens-form return carrier.* Every computed return type
  is a bare `TypeExprRef` token or a `:(…)` / dotted `SigiledTypeExpr`, so the
  `KType::KExpression` return-slot overload on each binder has nothing left to admit
  and is deleted — return-type dispatch keeps only the bare and sigil carrier shapes.

**Directions.**

- *Delete the FN / FUNCTOR `KExpression`-return overload — open.* The
  `KExpression`-return overload on each binder admits a parens-form `-> (expr)`
  computed return, along with the `ExprCarrier` / `ExprToSubDispatch` return-type path
  ([`src/builtins/fn_def/return_type.rs`](../../src/builtins/fn_def/return_type.rs)).
  The param-referencing-deferral substrate is shipped: a `:SigiledTypeExpr` return slot
  (the lazy sibling of `:KExpression`) captures a `:(…)` / dotted return raw and routes
  it through the existing `DeferredReturn::Expression` per-call channel. So deleting the
  overload means re-homing its remaining clients — computed `WITH` returns (still
  parenthesized `-> (Set WITH {Elt = Er.Type})` after Phase 3) and the bare-parens cases
  in `fn_def/tests/body_routing.rs` — onto the `:(…)` sigil carrier (`-> :(Set WITH {…})`),
  then removing the overload. Recommended: confirm the sigil form defers identically,
  migrate the clients, delete.
- *Phasing — decided.* Four green increments, (1)–(3) **shipped**: (1) the ATTR
  RHS-desugar, chained-access admission, the `:SigiledTypeExpr` lazy return carrier, and
  `M.T` retiring `MODULE_TYPE_OF`; (2) `LIST_OF` / `DICT_OF` → `:(LIST OF)` / `:(MAP ->)`;
  (3) `SIG_WITH` → infix `WITH` with record-literal bindings; (4) re-home the remaining
  parens-form computed returns onto the `:(…)` sigil carrier and delete the
  `KExpression`-return overload.
- *Declarators, named constructs, and `TEMPLATE` — decided (out of scope).*
  Binding-introduction declarators (`LET` / `FN` / `STRUCT` / `UNION` / `SIG` /
  `MODULE` / `NEWTYPE` / `VAL`) keep their lead keyword — it marks a not-yet-bound
  name the parser cannot infer from a left-hand value. Named constructs
  (`MATCH` / `TRY` / `USING … SCOPE` / `PRINT` / `CATCH`) keep theirs — the
  keyword is the English verb. `TEMPLATE` carries no underscore and names a
  higher-kinded constructor; it stays as is.

## Dependencies

**Requires:**

- [Type language via dispatch](../../design/typing/type-language-via-dispatch.md)
  — the `:(...)` type constructors (`LIST OF`, `MAP -> `) and the `SigiledTypeExpr`
  return carrier that the plain-English surfaces ride.

**Unblocks:**

- [Scheduler run/frame lifetime split](../refactor/scheduler-lifetime-split.md) — Phase 4's
  deletion of the `KExpression`-return overload removes a per-call-scope capture at the FN
  return boundary (`invoke.rs:180-186`) that the lifetime split would otherwise have to carry.
