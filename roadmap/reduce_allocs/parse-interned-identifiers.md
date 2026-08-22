# Parse-interned identifiers

**Problem.** The parser classifies every token (`classify_atom`,
[src/parse/tokens.rs](../../src/parse/tokens.rs)) but carries `Identifier` and `Type` parts
as bare text. Every downstream seam re-derives the symbol from that text at runtime —
declaration doors run `BinderSymbol::declared` / `TypeSymbol::declared`, and lookup seams
mint probe symbols per resolve ([src/machine/core/scope/resolve.rs](../../src/machine/core/scope/resolve.rs))
— so a name's BLAKE3 digest is recomputed at seams the parser already visited, and the
label interner is populated piecemeal at runtime declaration sites instead of at parse,
where the text is first in hand. The keyword vocabulary no longer works this way: keyword
parts carry their `KeywordSymbol`, minted and interned at the parse boundary.

**Acceptance criteria.**

- `Identifier` and `Type` parts carry their classified symbol beside the text, minted and
  interned where the parser classifies the token — the same shape keyword parts take.
- Declaration and lookup seams read the carried symbol instead of re-deriving it from
  text; runtime `Symbol::of` minting on name paths is confined to names that do not exist
  at parse (dynamic field access, metaprogramming-constructed expressions).
- The recorded allocation baselines do not regress, and the parse/declaration-side drop is
  visible in a measured figure.

**Directions.**

- *Which symbol class each part carries — open.* `Type` parts are unambiguously
  `TypeSymbol`; a bare `Identifier` is `ValueSymbol` by token class, but binder-position
  names route through `BinderSymbol` — whether the part carries the classified newtype or
  a `BinderSymbol` decided per position is the design fork.

## Dependencies

**Requires:** none — the parse-time interner plumbing and the carried-symbol part shape
this item extends are shipped substrate ([design/label-interning.md](../../design/label-interning.md)).

**Unblocks:** none.
