# Parse-interned identifiers

**Problem.** The parser classifies a value token as an `Identifier` (`classify_atom`,
[src/parse/tokens.rs](../../src/parse/tokens.rs)) and carries it as bare text —
`ExpressionPart::Identifier(&'a str)` ([src/machine/model/ast.rs](../../src/machine/model/ast.rs)).
The value-lookup ladder re-derives the `ValueSymbol` from that text at the top of every resolve
(`resolve_value_delivered`, [src/machine/core/scope/resolve.rs](../../src/machine/core/scope/resolve.rs)),
i.e. once per identifier per evaluation. At the bind seam the token is lowered to a
`KObject::KString` (`ExpressionPart::resolve`), so every binder builtin — `LET`, `VAL`, the
combined `LET … = FN`, `MODULE`, `GROUP`, `OP`, `ATTR` — reads its name back as text and mints
`ValueSymbol::declared` / `BinderSymbol::declared` again, and the statement's binder plan
(`StoredBinderKey::name`, [src/machine/model/binder.rs](../../src/machine/model/binder.rs)) stores
`(&str, BindKind)` and re-classifies in `to_owned_key`. Type tokens carry their symbol from the
parse boundary; Identifier tokens do not.

**Acceptance criteria.**

- `ExpressionPart::Identifier` carries a `ValueSymbol` and nothing else; the parser mints and
  interns it where it classifies the token. `FieldSlot::Name` carries the `ValueSymbol`.
- An `IDENTIFIER`-typed slot binds the token as `Held::Identifier(ValueSymbol)` — the mirror of
  `Held::UnresolvedType` — read through `BoundArgs::identifier(slot)`; the `Identifier` / `Type`
  arms of `ExpressionPart::resolve` / `resolve_region_pure` are unreachable. Every binder builtin
  reads the carried symbol; the value-lookup ladder takes a `ValueSymbol`; `BinderNameFn` yields a
  `BinderSymbol`; `StoredBinderKey::name` holds a `BinderSymbol` and `to_owned_key` mints nothing.
  A module's or group's child-scope name is stored as a symbol and rendered through the interner.
- `ATTR` has one `<s :Any> <field :Str>` overload for dynamic member access: a string-valued
  field operand — a literal (`ATTR s "x"`) or a computed value (`ATTR s (name_var)`) — reaches
  the member of `s` whose label is that text, deriving a bare `Symbol` through `labels.intern`
  (and a class through `BinderSymbol::declared`). A bare field token always binds bare through
  the `IDENTIFIER` overload — the `.` sugar (`s.x`) and spelled `ATTR s x` alike — even when
  that token is also a local binding holding a string.
- `ValueSymbol::of(text)` and `BinderSymbol::of(text)` are deleted: a classified symbol is born
  only at a declaration (`declared`) — the parser, a Rust-side static name, or a rendered builtin
  name — and a consumer that takes runtime text derives a bare `Symbol`.
- `symbols_minted` drops on every shape that resolves or binds a name; the recorded allocation
  baselines do not regress; `tests/allocation_baseline.rs` stays green.

**Directions.**

- *Bind-seam carrier — decided.* `Held::Identifier(ValueSymbol)`, never an aggregate cell (same
  disposition as `UnresolvedType` at every `Held` match). Keeping the `KString` lowering and
  re-classifying at the doors was rejected: it leaves a per-declaration hash on a path the parser
  already visited.
- *ATTR on runtime text — decided.* An `IDENTIFIER` slot admits only a parse token, so today's
  `KString` arm in `read_field_name` is the lowering artifact and goes away with the bare carrier.
  Dynamic access is a separate, explicit `field :Str` overload — the one derived-symbol door on
  `ATTR` — so the bare/derived split is visible in the overload table rather than hidden in a
  lowering. One overload, `<s :Any> <field :Str>`, reusing the `access_field` ladder; no per-lhs
  `Str` family.
- *Bare token vs resolved string — decided.* `KType::IDENTIFIER` ranks more specific than
  `KType::STR` (and `STR` not more specific than `IDENTIFIER`) in `is_more_specific_than`, so a
  bare field token shadowed by a local string binding still picks the `IDENTIFIER` overload — an
  Identifier slot claims the token itself, a `Str` slot only a resolved value. A dispatch
  admission carve-out was rejected: the ranking alone decides, and the pair has no other consumer
  (`Identifier` is not user-spellable and no other builtin bucket pairs the two).

## Dependencies

**Requires:** none — the type-side mirror, whose rendering seam and ladder shape this
reuses, has shipped.

**Unblocks:**

- [Symbol-keyed field lists](symbol-keyed-field-lists.md) — field names and record keys are
  Identifier-or-Type parts, so they need both part kinds symbol-only.
