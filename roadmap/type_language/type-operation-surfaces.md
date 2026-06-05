# Plain-English type-operation surfaces

Retire the `type_ops.rs` underscore-keyword family into the spaced, dotted, and
infix forms that already express the same operations, leaving no underscore
keyword and no redundant leading-keyword type operation.

**Problem.** The type-operation builtins in
[`src/builtins/type_ops.rs`](../../src/builtins/type_ops.rs) — `LIST_OF`,
`DICT_OF`, `MODULE_TYPE_OF`, `SIG_WITH` — each use an underscore-compound keyword
and lead the expression with it, outputting `KType::TypeExprRef`. These are the
only underscore keyword tokens in the language. Three of them duplicate a more
elegant surface that already exists:

- `LIST_OF T` against `:(LIST OF T)` (`src/builtins/type_constructors.rs:319`);
- `DICT_OF K V` against `:(MAP K -> V)` (`src/builtins/type_constructors.rs:326`);
- `MODULE_TYPE_OF M T` against `M.T`. The ATTR Type–Type overload resolves a
  module's type member (`src/builtins/attr.rs:236`), but `build_attr`
  (`src/parse/operators.rs:69`) is context-blind — it wraps every `.` in a
  value-context `Expression` returning `KType::Any`, so `M.T` cannot flow into a
  `TypeExprRef` / `Type` slot the way `MODULE_TYPE_OF`'s `KType::TypeExprRef` output
  can.

The fourth, `SIG_WITH sig (bindings)`, has no value-led equivalent. The
underscores read against koan's plain-English goal, and the leading keyword is
redundant wherever a spaced, sigil, or infix form already names the operation.

Spelling these as value-led expressions also costs a parallel dispatch surface.
A parens-form return type — `-> (MODULE_TYPE_OF Er Type)`, `-> (SIG_WITH …)` — is
an `Expression` part, so FN and FUNCTOR each register a *second* return-type
overload whose signature slot is `KType::KExpression` purely to admit it
([`src/builtins/fn_def.rs`](../../src/builtins/fn_def.rs),
[`src/builtins/functor_def.rs`](../../src/builtins/functor_def.rs)); the
`TypeExprRef`-return overload alone admits the `:(…)` and dotted forms.

**Impact.**

- *Type operations read as plain English.* `LIST_OF` / `DICT_OF` /
  `MODULE_TYPE_OF` are spelled with the existing spaced (`:(LIST OF)`,
  `:(MAP ->)`) and dotted (`M.T`) forms; `SIG_WITH` reads `sig WITH (bindings)`.
- *One surface per type operation.* The `TypeExprRef` family folds into the
  `:(...)` Type forms and the `.` operator, so each operation has a single
  spelling and no underscore keyword token remains in the language.
- *`SIG_WITH` joins the value-led operators.* It dispatches through the same
  `OperatorChain` substrate as `AS` / `FROM` / `:|` / `:!`, so it reads
  left-to-right from the signature it specializes.
- *FN and FUNCTOR carry one return-type carrier.* Every computed return type is a
  `:(…)` / dotted type-language form, so the `KType::KExpression` return-slot
  overload on each binder has nothing left to admit — return-type dispatch
  collapses to the single `TypeExprRef` carrier shape.

**Directions.**

- *ATTR desugars by RHS token class — decided.* `build_attr`
  (`src/parse/operators.rs:69`) branches on the right operand: a `Type(_)` field —
  a type member — folds to `SigiledTypeExpr([ATTR lhs rhs])`, a type-context
  carrier, while a value field stays the `Expression` form. This is the linchpin
  that makes `M.T` a type operation and lets `-> M.T` ride the `TypeExprRef`-return
  overload — so the `KExpression`-return overload below has nothing left to admit.
- *Retire the redundant underscore ops — decided.* `LIST_OF` → `:(LIST OF T)`,
  `DICT_OF` → `:(MAP K -> V)`, `MODULE_TYPE_OF` → `M.T`. Delete the three
  `type_ops.rs` registrations, following the `FUNCTION_OF` → `:(FN …)`
  retirement precedent recorded in the shipped notes.
- *Chained `Outer.Inner.x` reconciliation — open.* Left-assoc folding
  (`src/parse/tokens.rs:151`) makes the inner `Outer.Inner` a `SigiledTypeExpr`, so
  the outer ATTR's `s` slot receives a sub-dispatched `KTypeValue(Module)`. The
  `s :TypeExprRef` overload (`src/builtins/attr.rs:331`) must admit that carrier;
  confirm or add the admission before relying on the new desugar, covering both a
  value tail (`Outer.Inner.x`) and a type tail (`Outer.Inner.T`).
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
  `(LIST_OF (MODULE_TYPE_OF M T))`) accepts the `Type`-producing forms, or bridge
  the two, before deleting. Recommended: prove the slot types unify so no
  assembly path regresses.
- *Delete the FN / FUNCTOR `KExpression`-return overload — open.* Once no value-led
  type op survives, the parens-form `-> (expr)` return surface is unused; the
  second overload on each binder folds away, along with the `ExprCarrier` /
  `ExprToSubDispatch` return-type path
  ([`src/builtins/fn_def/return_type.rs`](../../src/builtins/fn_def/return_type.rs)).
  Gated on the deferral half of the reconciliation above: a param-referencing
  computed return (`-> Er.Type`, `-> Set WITH ((Elt Er.Type))`) must keep
  deferring per-call. Bare `-> Er` already defers through the `TypeExprRef`
  carrier (`TypeExprCarrier → Deferred`); confirm the dotted and `WITH` forms
  carry as deferrable `TypeExpr` carriers, not eager sub-dispatches, before
  removing the overload. After the ATTR change `-> Er.Type` arrives as a
  `SigiledTypeExpr` on the *eager* `TypeExprRef` return slot, so the deferral must
  intercept a param-referencing sigil before it sub-dispatches — either surgically
  (capture-and-defer just that case) or by making the return slot lazy and unifying
  all return-type carriage in the `return_type` pipeline. Recommended: prototype the
  surgical path first.
- *Phasing — decided.* Four green increments: (1) the ATTR RHS-desugar +
  chained-access admission + `M.T` retiring `MODULE_TYPE_OF`; (2) `LIST_OF` /
  `DICT_OF` → `:(LIST OF)` / `:(MAP ->)`; (3) `SIG_WITH` → infix `WITH`; (4) delete
  the `KExpression`-return overload. Phase 4 is gated on 1 + 3 removing both
  value-led parens-form clients.
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
