# Error-handling surface follow-ups

Surface-level error-handling work deferred while the dispatcher's structured
`KError` plumbing landed. Each bullet is a small, mostly-independent item; they
share this file because the type-system and dispatcher decisions for one
constrain the others. See [design/error-handling.md](../design/error-handling.md)
for the shipped behavior these extend.

- **Errors as first-class values.** `KObject::Err` would let errors bind via `LET` and
  pass as args. Needs the dispatcher to either short-circuit through error-typed slots
  or splice errors into them.
- **Catch-builtins** (`MATCH`, `OR_ELSE`-style). Likely require either a `KType::Result`
  extension or an `Argument.catches_errors` flag, which intersects with user-defined
  types in the module system.
- **`RAISE "msg"` builtin** to produce `KError::User` from in-language code.
- **Source spans on `KExpression`** so error frames can name `file:line` instead of
  textual summaries.
- **Continue-on-error after the first top-level failure** (useful for a future REPL).

## Dependencies

No hard prerequisites. The catch-builtins entry intersects with the
[module system](../design/module-system.md) — a `Result`-shaped signature would be the
natural carrier for the catch surface — but does not strictly require it.
