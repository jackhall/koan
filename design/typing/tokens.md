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
  `my_var`, `_internal`, `int_ord`). A bare `_` is not an identifier: with
  no letters it is a pure-symbol token, keyword-class — the wildcard
  TRY/MATCH match on, and the hole a `CLOSE OVER` capture pattern writes in
  each slot position ([lazy-closures.md](../lazy-closures.md)).

This split is what lets the language reserve a syntactic slot for type names
without quoting. `FN (x :Number) -> Str = (...)` works because `Number` and
`Str` are recognizable as types from their shape alone.

## Token class is a binding rule, not just a lexical one

The class a name lexes as decides **which universe it binds into**, and that is enforced, not
conventional. The type map and the value map are different universes — a Type token names
something that can type a field, a value token names something a field can hold — and each
keys by a symbol newtype minted only from text of its own class
([label-interning.md § Classified label vocabulary](../label-interning.md#classified-label-vocabulary)),
so a crossing is unrepresentable past the point where the name's text is classified.
That classification happens at the declaration seam, and a name that will not classify into
the channel it binds into is a hard error there:

- a value token entering the type channel — *"`int_ord` is a value token, so it names a value — a
  type binds under a Type token"*;
- a Type token entering the value channel — *"`IntOrd` is a Type token, so it names a type — a
  value binds under a value token (snake_case)"*.

A keyword-class name (all-uppercase, no lowercase) classifies into neither bindable
channel: **nothing binds to a keyword**, so an all-caps name can hold no value binding and
no type binding. Keyworded dispatch registration is unaffected — an `FN` or `OP`
registration labels a bucket in the dispatch table rather than binding a name.

The rule reaches past declarations to **parameters**: a
parameter's *name* picks its universe, not the argument it is handed, so a `:Type` /
`:Signature` parameter spells as a Type token (`Ty`, `Er`) and a module-valued parameter
spells snake_case (`er`); handing a module to a Type-token parameter raises the same
partition diagnostic at the frame bind. The partition admits no exception: a SIG body's
value slots (`VAL <name> :Type`) are recorded off the binding map entirely, in the decl
scope's own slot collector, so no value token ever lands in the type map (see
[elaboration.md § Binding-map partition](elaboration.md#binding-map-partition)).

A token that starts uppercase but classifies as neither keyword nor type
(e.g. a single uppercase letter `A`, or `K9`) is a parse error rather than
falling through to identifier — the rule keeps the type-position slot
syntactically discriminable and prevents a future binding from silently
shadowing a one-letter type-position identifier.

## A binder position is a name

A **binder position** — the `name` of `LET` / `NEWTYPE` / `UNION` / `SIG` / `TYPE` /
`MODULE` / `GROUP`, the `field` of `ATTR` — denotes a name, never a type reference, so it
never resolves: not against the builtin type table, not against scope. Its class is the class
the parser already assigned the token, taken from the part variant and never re-derived by a
predicate over rendered text.

Three slot types express that, and none of them is user-spellable:

| Slot | Admits | Delivered as |
|---|---|---|
| `Identifier` | an `Identifier` part | `Held::Name(BinderSymbol::Value(_))` |
| `NameToken` | an `Identifier` *or* `Type` part | `Held::Name(_)`, class per part variant |
| `TypeNameToken` | a `Type` part | `Held::Name(BinderSymbol::Type(_))` |

None is an `OfKind(_)`, so the bind seam's builtin-table lowering
([`KType::from_symbol`](../../src/machine/model/types/ktype_resolution.rs)) is never consulted
for a binder, and a binder never rides `Held::UnresolvedType` — that carrier means "a type
reference awaiting scope resolution", which a binder is not. The consequence at the surface is
uniformity: a name that happens to spell a builtin type differs from a fresh one in exactly one
way, whether it is already bound. `LET Str = Number`, `LET List = Number` and
`LET Dict = Number` all report the same `Rebind`, and no diagnostic quotes a lowered type in
place of the token the user wrote.

The classes stay disjoint where a builtin wants to tell them apart: `MODULE` and `GROUP` keep an
`Identifier` overload that binds and a `TypeNameToken` twin that raises the snake_case respelling
diagnostic, naming the token as written.

## The module system adds no fourth class

The [module system](modules.md) adds no fourth class; it splits along the
existing seam. A **signature** is a type, so signature names (`Ordered`,
`Showable`) take the Type class alongside host type names, and the
discrimination between "host type" and "signature" happens at scope resolution,
not at lex time. A **module** is a value, so module names (`int_ord`, `int_set`)
take the Identifier class: `MODULE` requires one, and the Type class is thereby
exactly the set of names that can type a field. A `.`-compound on a module name
resolves to module member access the same way a `.`-compound on a struct value
resolves to a field read, and a module-qualified `int_ord.Carrier` in type position
parses as a single `TypeName` leaf. Abstract type declarations inside a signature
use the Type-class spelling — the convention is `Carrier` for the principal
abstract type, with `Elt`, `Key`, `Val` etc. when more than one is needed.

A bare module name is therefore never a type: `:int_ord` fails at the `:` sigil,
whose next token must be a Type token. The `TYPE OF` builtin is the door from a
value to its type (see
[modules.md § Modules in type position](modules.md#modules-in-type-position-type-of)).

## Open work

- [Name-token slots for binder positions](../../roadmap/type_language/name-token-slots.md) —
  the combined `LET <Name> = FN …` statement's binder is the one position still typed as a type
  reference, so its diagnostic quotes a lowered handle rather than the token as written.
