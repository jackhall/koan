# Type-expression sigil — replace `<>` brackets with glued `:`

Replace `List<Number>` / `Dict<K, V>` / `Function<(A) -> R>` with a glued-right `:`
sigil opening an S-expression type group: `:(List Number)` / `:(Dict K V)` /
`:(Function (A) -> R)`. Free `<`, `>`, `<=`, `>=` for arithmetic.

**Problem.** Type-parameter syntax today is the `<...>` form folded by
[`parse/type_frame.rs`](../src/parse/type_frame.rs) and the `<`/`>` arms in
[`parse/expression_tree.rs`](../src/parse/expression_tree.rs), with adjacency rules
([`check_separator_adjacency`](../src/parse/expression_tree.rs)) keeping `<` from
gluing to a non-Type token. That occupies `<` and `>` lexically — any future math
operator (`<`, `>`, `<=`, `>=`) collides with type-position parsing. The S-expression
forms shipped by the module system (`MODULE`, `STRUCT`, `TAGGED`, `NEWTYPE`, `SIG`,
`LET Type = …`) leave a single asymmetry: parameterized type *annotations* still ride
the `<>` form, even though every other type-bearing surface is S-expr.

**Impact.**

- *Arithmetic comparison operators become available.* `<`, `>`, `<=`, `>=` are free
  to be wired up as math operators with no special-case parser arms.
- *One surface rule for type position.* Every type expression — bare, parameterized,
  function-typed — is preceded by a glued-right `:`. Ascription (`xs :Number`), return
  type (`-> :Number`), type-arg slots (`:(Dict Str :(List Number))`), and LET type
  binding (`LET t = :(List Number)`) all use the same lexical marker, so the parser
  knows type-vs-value lexically rather than contextually.
- *LET type-binding ambiguity dissolves.* `LET t = :(List Number)` is unambiguously
  a type binding because the RHS is sigil-prefixed; the contextual "LHS is a Type
  token" rule that today's modules rely on (e.g. `LET Type = Number`) becomes
  redundant.
- *Net parser deletion.* `parse/type_frame.rs`, the `Frame::Type` variant, the `<`/`>`
  arms, and the separator-adjacency helper all leave the codebase.

**Directions.**

- *S-expression annotation surface — decided.* `:(List Number)`, `:(Dict K V)`,
  `:(Function (A B) -> R)`. The `:` sigil opens a type-expression group whose contents
  parse in type mode; nested `(...)` are nested type expressions; tokens are type
  names. Per `scratch/type_sigil_redesign.md`.
- *Function-type form — decided.* Shape (b): `:(Function (Number Str) -> Bool)` —
  keep the `->` arrow as the args-to-return separator inside type-expression mode.
  Outside type position, `->` keeps its current FN-signature role unchanged.
- *Bare-type sigil requirement — decided as "looser" (B1), permanent.* `xs :Number`
  is the new ascription form, but bare `Number` tokens outside a sigil context still
  classify as `ExpressionPart::Type` so existing module forms (`LET Type = Number`)
  keep working without per-token rewrite. The sigil is *required* for parameterized
  types and *optional* for bare types. B1 is the endpoint, not a stepping stone:
  - *No semantic payoff to tightening.* Consumer audit confirmed every type-position
    site uses only `TypeExpr.name` / `params`; none branch on whether the user wrote
    the sigil. With no first-class type-as-value distinction in koan, `:Number` and
    `Number` carry identical information post-parse.
  - *Readability.* `LET Type = Number` and `STRUCT Point = (x: Number, y: Number)`
    read more naturally without obligatory `:` clutter on every bare type token.
    Forcing the sigil where it adds no information would be teaching noise.
  - *B2 is dropped, not deferred.*
- *Dict-vs-sigil `:` disambiguation — decided.* Inside a Dict frame, `:` stays the
  pair separator (unchanged). Outside a Dict frame: `:|` and `:!` keep their
  module-ascription roles; `:` glued-right (no whitespace before the next token or
  `(`) opens a type expression; lone `:` outside dict context is an error.
- *Error vocabulary for `:` glue violations — open.* `xs : Number` (space after the
  colon) under the new rule is a parse error. Diagnostic wording needs to point
  users at the fix (`xs :Number`). Candidates: "':' must be glued to its operand at
  a type position", "remove the space between ':' and the type". Pick during
  implementation. Recommended: the explicit fix-it form.
- *Stricter B2 tightening — decided against.* Requiring the sigil on bare types
  (so `LET t = Number` becomes `LET t = :Number`) is dropped, not deferred.
  Consumer audit found no site that would act on the distinction, and bare-type
  readability (`LET Type = Number`, `STRUCT Point = (x: Number, y: Number)`) is a
  positive reason to keep the asymmetry rather than tolerate it. See
  [`scratch/type_sigil_redesign.md`](../scratch/type_sigil_redesign.md) §R2.

The remaining items are scoped sub-tasks for implementation rather than design
choices. Full per-phase breakdown lives in
[`scratch/type_sigil_redesign.md`](../scratch/type_sigil_redesign.md).

- *Parser changes.* Recognize glued-right `:` outside Dict frames; open a new
  `Frame::TypeExpr` whose close folds into `ExpressionPart::Type` with the same
  `TypeExpr { name, params }` shape today's `TypeFrame::build` produces. The resolver
  ([`src/runtime/machine/model/types/resolver.rs`](../src/runtime/machine/model/types/resolver.rs))
  consumes the unchanged `TypeExpr`, so all elaboration paths keep working.
- *Consumer rewrite.* Six type-position consumers
  ([`fn_def/signature.rs`](../src/runtime/builtins/fn_def/signature.rs),
  [`let_binding.rs`](../src/runtime/builtins/let_binding.rs),
  [`value_lookup.rs`](../src/runtime/builtins/value_lookup.rs),
  [`argument_bundle.rs`](../src/runtime/machine/core/kfunction/argument_bundle.rs),
  [`type_ops.rs`](../src/runtime/builtins/type_ops.rs),
  [`fn_def.rs`](../src/runtime/builtins/fn_def.rs)) drop their
  `Keyword(":")` separator and consume the `ExpressionPart::Type` directly.
- *Old-machinery deletion.* `parse/type_frame.rs`, the `Frame::Type` variant in
  [`parse/frame.rs`](../src/parse/frame.rs), the `<`/`>` arms in
  [`parse/expression_tree.rs`](../src/parse/expression_tree.rs), and
  `check_separator_adjacency` all go.
- *Fixture migration.* ~35 source/test files reference `List<...>` / `Dict<...>` /
  `Function<...>`; bulk-rewriteable by shape. Every `name: Type` ascription also
  shifts to `name :Type`.
- *Doc migration.* `README.md`, `TUTORIAL.md`, `design/type-system.md`,
  `design/module-system.md`, `design/effects.md`, `design/functional-programming.md`,
  plus four roadmap items reference `<>` shapes — all need updating.

## Dependencies

**Requires:** none — self-contained parser refactor.

**Unblocks:** none — no current roadmap item depends on `<`, `>`, `<=`, `>=`
becoming available, but the work is a prerequisite for any future numeric
comparison or relational operator surface.
