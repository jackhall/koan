# Quote and eval sigils

Two prefix sigils that make the lazy/eager split between expressions and dict/list
literals navigable from in-language code.

**Problem.** Whether a sub-expression is evaluated or held as raw AST is determined by
*context* today: a slot typed `KType::KExpression` consumes the raw `ExpressionPart`
unevaluated, while a dict or list literal eagerly evaluates each entry. This works as
long as those defaults match what the user wants. Two symmetric gaps remain:

- *No way to force-evaluate a metaexpression in a `KExpression` slot.* If a value is a
  `KObject::KExpression` (a "metaexpression" — produced by some computation, not
  surface-typed at the parse site), there is no surface form that says "treat this as the
  AST to consume," only positions that would consume the AST surface-statically.
- *No way to suppress evaluation inside a dict or list literal.* The scheduler in
  [src/execute/run.rs:205-213](../src/execute/run.rs#L205-L213) wraps every
  bare-identifier dict entry in a `value_lookup` dispatch — Python-like name resolution
  applies even on the key side. There is no way to say "this identifier should remain a
  literal symbol for the dict's purposes" or "this sub-expression should be captured as
  a `KObject::KExpression` value." This is what blocks the otherwise-natural
  `Point {x: 3, y: 4}` surface for struct construction (today shipped as
  `Point (x: 3, y: 4)` with the named-arg refactor) — without quote, `x` and `y` get
  scope-looked-up.

**Impact.** Today the limitation is mostly invisible because the contexts that need it
(meta-programming, dict-as-named-args, lazy struct fields) either don't exist yet or have
been built around the gap. The named-args refactor inherits but does not regress this —
it stays in expression-triple form (`(x: 3, y: 4)`) precisely because the dict form
would trip on (2). As more meta-programming surface lands (effect handlers, trait method
dispatch, user-extensible types, `EVAL`-style builtins), each one will either need the
sigils or grow its own bespoke escape.

**Direction (sketch, not committed).** A symmetric prefix-operator pair, sitting in the
existing `OPERATORS` table in [src/parse/operators.rs](../src/parse/operators.rs):

- `` `expr `` — *quote*: the expression's AST is captured as a `KObject::KExpression`
  value with no evaluation. Lets the user thread raw ASTs through eager-evaluating
  contexts (dict values, list elements, function args).
- `$expr` — *eval*: the expression resolves to a value and that value, if a
  `KObject::KExpression`, is then evaluated as an AST in the current scope. Dual to
  quote — lets the user thread KExpression values through lazy slots that would
  otherwise consume the raw AST.

Backtick is unused today and reads as "literal/raw" in most language traditions; `$`
already carries shell template-substitution baggage so `$expr` parses as "value of."
Both fit the existing prefix-operator pattern (one trigger char + the following compound
token) so the parser change is a one-row add to the registry. The runtime side needs a
`BuiltinFn`-shaped quote that wraps its argument in `KObject::KExpression` and an eval
that pulls the inner AST out and re-dispatches.

## Dependencies

Cleanly ships any time. Useful immediately for the dict-as-struct-args ergonomic upgrade
if/when that gets prioritized over today's expression-triple surface, and as the
foundation for an in-language `EVAL` builtin and lazy struct field types.
