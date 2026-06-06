# Plain-English type-operation surfaces

Retire the `type_ops.rs` underscore-keyword family into the spaced, dotted, and
infix forms that already express the same operations, leaving no underscore
keyword and no redundant leading-keyword type operation.

**Problem.** The remaining type-operation builtins in
[`src/builtins/type_ops.rs`](../../src/builtins/type_ops.rs) — `LIST_OF`,
`DICT_OF`, `SIG_WITH` — each use an underscore-compound keyword and lead the
expression with it, outputting `KType::TypeExprRef`. These are the only underscore
keyword tokens left in the language. Two of them duplicate a more elegant surface
that already exists:

- `LIST_OF T` against `:(LIST OF T)` (`src/builtins/type_constructors.rs:319`);
- `DICT_OF K V` against `:(MAP K -> V)` (`src/builtins/type_constructors.rs:326`).

The third, `SIG_WITH sig (bindings)`, has no value-led equivalent. The
underscores read against koan's plain-English goal, and the leading keyword is
redundant wherever a spaced, sigil, or infix form already names the operation.

Spelling these as value-led expressions also costs a parallel dispatch surface.
A parens-form return type — `-> (SIG_WITH …)` — is an `Expression` part, so FN and
FUNCTOR each register a return-type overload whose slot is `KType::KExpression`
purely to admit it ([`src/builtins/fn_def.rs`](../../src/builtins/fn_def.rs),
[`src/builtins/functor_def.rs`](../../src/builtins/functor_def.rs)); the
`TypeExprRef` and `SigiledTypeExpr` return overloads admit the bare and `:(…)` /
dotted forms. `SIG_WITH` is the lone remaining client of the `KExpression`-return
overload, so retiring it (Phase 3) clears the way to delete that overload (Phase 4).

**Impact.**

- *Type operations read as plain English.* `LIST_OF` / `DICT_OF` are spelled with
  the existing spaced forms (`:(LIST OF)`, `:(MAP ->)`); `SIG_WITH` reads
  `sig WITH (bindings)`. (A module type-member already reads as the dotted `M.T`.)
- *One surface per type operation.* The `TypeExprRef` family folds into the
  `:(...)` Type forms and the `.` operator, so each operation has a single
  spelling and no underscore keyword token remains in the language.
- *`SIG_WITH` joins the value-led operators.* It dispatches through the same
  `OperatorChain` substrate as `AS` / `FROM` / `:|` / `:!`, so it reads
  left-to-right from the signature it specializes.
- *FN and FUNCTOR drop the parens-form return carrier.* Every computed return type
  is a bare `TypeExprRef` token or a `:(…)` / dotted `SigiledTypeExpr`, so the
  `KType::KExpression` return-slot overload on each binder has nothing left to admit
  and is deleted — return-type dispatch keeps only the bare and sigil carrier shapes.

**Directions.**

- *Retire the redundant underscore ops — decided.* `LIST_OF` → `:(LIST OF T)`,
  `DICT_OF` → `:(MAP K -> V)`. Delete the two remaining `type_ops.rs` registrations,
  following the `FUNCTION_OF` → `:(FN …)` and `MODULE_TYPE_OF` → `M.T` retirement
  precedents recorded in the shipped notes.
- *`SIG_WITH` → infix `WITH` — decided.* `sig WITH (bindings)` registers as a
  keyworded builtin with a leading-slot signature — `[arg sig, kw WITH, arg
  bindings]` — exactly like `FROM` (`src/builtins/record_projection.rs:99`) and
  `:|` / `:!` (`src/builtins/ascribe.rs:226`). A lone binary infix classifies as
  `Keyworded`, not `OperatorChain` (the chain shape needs ≥ 2 keyword positions,
  `src/machine/model/ast.rs:339`), so it needs nothing from the open n-ary operator
  fold — which is unimplemented and errors on every registry hit
  (`src/machine/execute/dispatch/operator_chain.rs:54`).
- *`TypeExprRef` / `Type` reconciliation — open.* The retired ops output
  `KType::TypeExprRef`; their replacements output `KType::Type` / `KTypeValue`,
  and `DICT_OF`'s `KType::Dict` must be confirmed equivalent to `MAP`'s output.
  Verify every type-assembly context that consumes `TypeExprRef` (e.g. nested
  `(LIST_OF (DICT_OF Str Number))`) accepts the `Type`-producing forms, or bridge
  the two, before deleting. Recommended: prove the slot types unify so no
  assembly path regresses.
- *Delete the FN / FUNCTOR `KExpression`-return overload — open.* Once `SIG_WITH`
  goes infix (Phase 3), the parens-form `-> (expr)` return surface is unused; the
  `KExpression`-return overload on each binder folds away, along with the
  `ExprCarrier` / `ExprToSubDispatch` return-type path
  ([`src/builtins/fn_def/return_type.rs`](../../src/builtins/fn_def/return_type.rs)).
  The param-referencing-deferral half is shipped: a `:SigiledTypeExpr` return slot
  (the lazy sibling of `:KExpression`) captures a dotted/sigil `-> Er.Type` raw and
  routes it through the existing `DeferredReturn::Expression` per-call channel, so the
  computed return keeps deferring without the parens form. Confirm the `WITH` form
  (`-> Set WITH ((Elt Er.Type))`) carries the same way once Phase 3 lands, before
  removing the overload.
- *Phasing — decided.* Four green increments: (1) **shipped** — the ATTR RHS-desugar,
  chained-access admission, the `:SigiledTypeExpr` lazy return carrier, and `M.T`
  retiring `MODULE_TYPE_OF`; (2) `LIST_OF` / `DICT_OF` → `:(LIST OF)` / `:(MAP ->)`;
  (3) `SIG_WITH` → infix `WITH`; (4) delete the `KExpression`-return overload, gated
  on 3 removing its last value-led parens-form client.
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
  — the `:(...)` constructors (`LIST OF`, `MAP -> `) and the `OperatorChain`
  infix substrate the replacement surfaces ride.

**Unblocks:**

- [Scheduler run/frame lifetime split](../refactor/scheduler-lifetime-split.md) — Phase 4's
  deletion of the `KExpression`-return overload removes a per-call-scope capture at the FN
  return boundary (`invoke.rs:180-186`) that the lifetime split would otherwise have to carry.

`SIG_WITH`'s infix form needs only the binary single-keyword `OperatorChain`
shape, which is shipped and already carries `FROM` / `AS` / `:|` / `:!` — it does
not depend on the open n-ary-operator fold.
