# Token classes — the parser-level foundation

The lexer ([tokens.rs](../../src/parse/tokens.rs)) splits non-literal atoms into
three classes:

- **Keyword** — pure-symbol tokens (`=`, `->`, `:|`, `:!`, `+`) and alphabetic
  tokens with **two or more uppercase letters and no lowercase letters**
  (`LET`, `THEN`, `MODULE`, `SIG`). Contribute fixed tokens to a signature's
  bucket key. The two-uppercase floor reserves single-letter capitals (`A`,
  `K`) and uppercase-plus-digits shapes (`K9`, `AB1`) as syntactic territory
  rather than letting them silently classify as identifiers — see below.
- **Type** — uppercase-leading with at least one lowercase letter elsewhere
  (`Number`, `Str`, `KFunction`, `MyType`, `Ordered`). Type references and
  signature names share this class.
- **Identifier** — lowercase-leading or `_`-leading names (`compare`,
  `my_var`, `_internal`, `int_ord`).

This split is what lets the language reserve a syntactic slot for type names
without quoting. `FN (x :Number) -> Str = (...)` works because `Number` and
`Str` are recognizable as types from their shape alone.

## Token class is a binding rule, not just a lexical one

The class a name lexes as decides **which universe it binds into**, and that is enforced, not
conventional. The type map and the value map are different universes — a Type token names
something that can type a field, a value token names something a field can hold — and
[`Bindings`](../../src/machine/core/bindings.rs)'s partition guard makes a crossing a hard
error at the write:

- a value token entering the type map — *"`int_ord` is a value token, so it names a value — a
  type binds under a Type token"*;
- a Type token entering the value map — *"`IntOrd` is a Type token, so it names a type — a
  value binds under a value token (snake_case)"*.

A keyword-class name (all-uppercase, no lowercase) is neither, so builtin dispatch
registration is unaffected. The rule reaches past declarations to **parameters**: a
parameter's *name* picks its universe, not the argument it is handed, so a `:Type` /
`:Signature` parameter spells as a Type token (`Ty`, `Er`) and a module-valued parameter
spells snake_case (`er`). The one exception is a SIG body's slot table, which keys value slots
by their value names inside the type map — a schema rather than a binding universe (see
[elaboration.md § Binding-map partition](elaboration.md#binding-map-partition)).

A token that starts uppercase but classifies as neither keyword nor type
(e.g. a single uppercase letter `A`, or `K9`) is a parse error rather than
falling through to identifier — the rule keeps the type-position slot
syntactically discriminable and prevents a future binding from silently
shadowing a one-letter type-position identifier.

The [module system](modules.md) adds no fourth class; it splits along the
existing seam. A **signature** is a type, so signature names (`Ordered`,
`Showable`) take the Type class alongside host type names, and the
discrimination between "host type" and "signature" happens at scope resolution,
not at lex time. A **module** is a value, so module names (`int_ord`, `int_set`)
take the Identifier class: `MODULE` requires one, and the Type class is thereby
exactly the set of names that can type a field. A `.`-compound on a module name
resolves to module member access the same way a `.`-compound on a struct value
resolves to a field read, and a module-qualified `int_ord.Type` in type position
parses as a single `TypeName` leaf. Abstract type declarations inside a signature
use the Type-class spelling — the convention is `LET Type = ...` for the principal
abstract type, with `Elt`, `Key`, `Val` etc. when more than one is needed.

A bare module name is therefore never a type: `:int_ord` fails at the `:` sigil,
whose next token must be a Type token. The `TYPE OF` builtin is the door from a
value to its type (see
[modules.md § Modules in type position](modules.md#modules-in-type-position-type-of)).
