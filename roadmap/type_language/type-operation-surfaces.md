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
- `MODULE_TYPE_OF M T` against `M.T` — the ATTR Type–Type overload already
  resolves a module's type member (`src/builtins/attr.rs:249`).

The fourth, `SIG_WITH sig (bindings)`, has no value-led equivalent. The
underscores read against koan's plain-English goal, and the leading keyword is
redundant wherever a spaced, sigil, or infix form already names the operation.

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

**Directions.**

- *Retire the redundant underscore ops — decided.* `LIST_OF` → `:(LIST OF T)`,
  `DICT_OF` → `:(MAP K -> V)`, `MODULE_TYPE_OF` → `M.T`. Delete the three
  `type_ops.rs` registrations, following the `FUNCTION_OF` → `:(FN …)`
  retirement precedent recorded in the shipped notes.
- *`SIG_WITH` → infix `WITH` — decided.* `sig WITH (bindings)` dispatches via the
  binary `OperatorChain` shape (the same single-keyword infix shape `FROM` uses),
  dropping both the underscore and the leading keyword.
- *`TypeExprRef` / `Type` reconciliation — open.* The retired ops output
  `KType::TypeExprRef`; their replacements output `KType::Type` / `KTypeValue`,
  and `DICT_OF`'s `KType::Dict` must be confirmed equivalent to `MAP`'s output.
  Verify every type-assembly context that consumes `TypeExprRef` (e.g. nested
  `(LIST_OF (MODULE_TYPE_OF M T))`) accepts the `Type`-producing forms, or bridge
  the two, before deleting. Recommended: prove the slot types unify so no
  assembly path regresses.
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

**Unblocks:** none tracked yet.

`SIG_WITH`'s infix form needs only the binary single-keyword `OperatorChain`
shape, which is shipped and already carries `FROM` / `AS` / `:|` / `:!` — it does
not depend on the open n-ary-operator fold. A leaf cleanup; it blocks no open
item.
