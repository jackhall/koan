# Other deferred surface items

Smaller pieces called out in passing as the larger items shipped:

- **Errors as first-class values.** `KObject::Err` would let errors bind via `LET` and
  pass as args. Needs the dispatcher to either short-circuit through error-typed slots
  or splice errors into them.
- **Catch-builtins** (`MATCH`, `OR_ELSE`-style). Likely require either a `KType::Result`
  extension or an `Argument.catches_errors` flag, which intersects with the
  user-defined-types work above.
- **`RAISE "msg"` builtin** to produce `KError::User` from in-language code.
- **Source spans on `KExpression`** so error frames can name `file:line` instead of
  textual summaries.
- **Continue-on-error after the first top-level failure** (useful for a future REPL).

## Dependencies

No hard prerequisites. The catch-builtins entry intersects with the open type-system
items ([per-type identity](per-type-identity.md) onward) but does not strictly require
them.
