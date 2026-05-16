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
  (`Number`, `Str`, `KFunction`, `MyType`, `IntOrd`, `OrderedSig`). Type
  references, module names, and signature names all share this class.
- **Identifier** — lowercase-leading or `_`-leading names (`compare`,
  `my_var`, `_internal`).

This split is what lets the language reserve a syntactic slot for type names
without quoting. `FN (x :Number) -> Str = (...)` works because `Number` and
`Str` are recognizable as types from their shape alone.

A token that starts uppercase but classifies as neither keyword nor type
(e.g. a single uppercase letter `A`, or `K9`) is a parse error rather than
falling through to identifier — the rule keeps the type-position slot
syntactically discriminable and prevents a future binding from silently
shadowing a one-letter type-position identifier.

The [module system](modules.md) reuses the Type class without adding
a fourth: module names (`IntOrd`, `MakeSet`) and signature names
(`OrderedSig`, `ShowableSig`) classify the same way as host type names. The
discrimination between "host type", "module", and "signature" happens at
scope resolution, not at lex time — a `.`-compound on a module-class token
resolves to module member access the same way a `.`-compound on a struct
value resolves to a field read, and a module-qualified `IntOrd.Type` in type
position parses as a single structured `TypeExpr`. Abstract type
declarations inside a signature use the Type-class spelling too — the
convention is `LET Type = ...` for the principal abstract type, with `Elt`,
`Key`, `Val` etc. when more than one is needed.
