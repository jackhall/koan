# Anonymous functions

Keyword-less function literals — `FN {x :Number} -> ReturnType = (body)` — that
evaluate to a plain function value, bound by `LET` or passed straight into a
function-typed slot, with no dispatch keyword.

**Problem.** Every FN signature is a parenthesized `KExpression` that must
contain at least one fixed `Keyword` token (`src/builtins/fn_def/signature.rs`).
That keyword is the dispatch key: `finalize_fn` registers the function in the
scope's dispatch table under it (`src/builtins/fn_def/finalize.rs`). There is no
surface for a function with *no* keyword. Passing a one-off function into a
function-typed parameter slot — which the type system already admits via
`KType::KFunction` (`src/machine/model/types/ktype.rs`) and contravariant
function subtyping (`src/machine/model/types/ktype_predicates.rs`) — forces
coining a throwaway keyword, e.g. `(USE (FN (SHOW x :Number) -> Str = ("hi")))`,
where `SHOW` carries no meaning yet still registers a sibling-visible dispatch
form.

**Impact.**

- *A keyword-less function literal.* `FN {x :Number} -> Str = (...)` evaluates
  to a function value with no dispatch key, bound via `LET` or dropped directly
  into a function-typed parameter slot.
- *Higher-order call sites read as the lambda they are.* Passing an inline
  function to a combinator no longer coins a meaningless keyword or adds a
  one-shot form to the dispatch table.
- *Anonymous functions interlock with what's already shipped.* The `{...}`
  binder produces the existing `KType::KFunction`, and the value rides the
  existing function-value call path, lexical closure capture, and contravariant
  subtyping — no new type or call machinery.
- *Substrate for ergonomic standard-library combinators.* The `Result`
  `map` / `bind` and collection `map` / `filter` / `fold` entries
  ([standard library](libraries/standard-library.md)) become idiomatic to
  call with an inline function rather than a pre-declared keyworded one.

**Directions.**

- *Record-schema binder — decided.* `FN {<record schema>} -> ReturnType =
  (<body>)` replaces the `(...)` keyword signature with a `{...}` record of
  `name :Type` fields; the absence of a keyword is what makes the function
  anonymous.
- *Parser reuse — decided.* The `{...}` schema parses through the shared
  `parse_typed_field_list_via_elaborator`
  (`src/machine/model/types/typed_field_list.rs`) that struct/union field lists
  and record types already use, not a bespoke path.
- *No dispatch registration — decided.* An anonymous FN registers no keyword in
  the scope; its only handle is the value it evaluates to. This is the defining
  difference from the keyworded form, which always registers its lead keyword.
- *Closure capture — decided.* Anonymous functions capture their defining scope
  lexically through the same `captured` scope and `Rc<CallArena>` escape path
  named FNs use (`src/machine/core/kfunction.rs`); no new capture machinery.
- *Calling convention — decided.* An anonymous function is called by passing a
  record literal whose named fields fill the schema — `f {x = 1}` — resolved
  through the `FunctionValueCall` fast lane
  (`src/machine/execute/dispatch/fn_value.rs`), with the record's fields mapped
  onto the signature slots by `KFunction::reconstruct_positional`.
- *Zero- and single-field schemas — decided.* Both forms parse: `FN {} -> ...`
  is a no-parameter thunk (called with the empty record `{}`), and the
  single-field `FN {x :Number} -> ...` is called with a one-field record
  `{x = …}`.
- *Relationship to inline keyworded FNs — decided.* Neither form is the
  recommended default; the choice is made per definition on (a) readability and
  (b) whether a dispatch keyword is meaningful for that function. The keyworded
  `(...)` form and the anonymous `{...}` form coexist.

## Dependencies

**Requires:** none at the roadmap level. Function types and function-typed
parameters (`KType::KFunction`), record-schema parsing, the `FunctionValueCall`
path, contravariant function subtyping, and lexical closure capture are all
shipped substrate.

**Unblocks:** none as a hard edge.

The relationship to the [standard library](libraries/standard-library.md) is
a soft one: its higher-order combinators can be *defined* today (they are
ordinary FNs over function-typed parameters), but they only become ergonomic to
*call* once a one-off function can be written without coining a dispatch
keyword. This item is a surface convenience over shipped substrate, so it has no
hard prerequisite and blocks nothing outright.
