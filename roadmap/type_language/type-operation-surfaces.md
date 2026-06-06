# Plain-English type-operation surfaces

Retire the `type_ops.rs` underscore-keyword family into the spaced, dotted, and
infix forms that already express the same operations, leaving no underscore
keyword and no redundant leading-keyword type operation.

**Problem.** The remaining underscore-keyword type-operation builtins in
[`src/builtins/type_ops.rs`](../../src/builtins/type_ops.rs) — `LIST_OF` and
`DICT_OF` — each use an underscore-compound keyword and lead the expression with it,
outputting `KType::TypeExprRef`. These are the only underscore keyword tokens left in
the language, and both duplicate a more elegant surface that already exists:

- `LIST_OF T` against `:(LIST OF T)` (`src/builtins/type_constructors.rs:319`);
- `DICT_OF K V` against `:(MAP K -> V)` (`src/builtins/type_constructors.rs:326`).

The underscores read against koan's plain-English goal, and the leading keyword is
redundant wherever a spaced or sigil form already names the operation.

A separate cost is the parens-form computed return type. `-> (… WITH {…})` (and any
`-> (expr)`) is an `Expression` part, so FN and FUNCTOR each register a return-type
overload whose slot is `KType::KExpression` purely to admit it
([`src/builtins/fn_def.rs`](../../src/builtins/fn_def.rs),
[`src/builtins/functor_def.rs`](../../src/builtins/functor_def.rs)); the `TypeExprRef`
and `SigiledTypeExpr` return overloads already admit the bare and `:(…)` / dotted
forms. The `SIG_WITH` → infix `WITH` migration (Phase 3) retired the underscore builtin
but kept its computed return parenthesized (`-> (Set WITH {…})`), so the
`KExpression`-return overload still has clients; re-homing those to the `:(…)` sigil
carrier (so every computed return is a bare token, `:(…)`, or dotted form) is what lets
Phase 4 delete the overload.

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

- *Retire the redundant underscore ops — decided.* `LIST_OF` → `:(LIST OF T)`,
  `DICT_OF` → `:(MAP K -> V)`. Delete the two remaining `type_ops.rs` registrations,
  following the `FUNCTION_OF` → `:(FN …)` and `MODULE_TYPE_OF` → `M.T` retirement
  precedents recorded in the shipped notes.
- *`TypeExprRef` / `Type` reconciliation — open.* The retired ops output
  `KType::TypeExprRef`; their replacements output `KType::Type` / `KTypeValue`,
  and `DICT_OF`'s `KType::Dict` must be confirmed equivalent to `MAP`'s output.
  Verify every type-assembly context that consumes `TypeExprRef` (e.g. nested
  `(LIST_OF (DICT_OF Str Number))`) accepts the `Type`-producing forms, or bridge
  the two, before deleting. Recommended: prove the slot types unify so no
  assembly path regresses.
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
- *Phasing — decided.* Four green increments: (1) **shipped** — the ATTR RHS-desugar,
  chained-access admission, the `:SigiledTypeExpr` lazy return carrier, and `M.T`
  retiring `MODULE_TYPE_OF`; (2) `LIST_OF` / `DICT_OF` → `:(LIST OF)` / `:(MAP ->)`;
  (3) **shipped** — `SIG_WITH` → infix `WITH` with record-literal bindings; (4) re-home
  the remaining parens-form computed returns onto the `:(…)` sigil carrier and delete the
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
  — the `:(...)` constructors (`LIST OF`, `MAP -> `) the `LIST_OF` / `DICT_OF`
  replacements lower to.

**Unblocks:**

- [Scheduler run/frame lifetime split](../refactor/scheduler-lifetime-split.md) — Phase 4's
  deletion of the `KExpression`-return overload removes a per-call-scope capture at the FN
  return boundary (`invoke.rs:180-186`) that the lifetime split would otherwise have to carry.
