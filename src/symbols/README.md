# Symbols

A symbol — a record field name, a struct schema field, an FN parameter name —
originates in source text and is fixed at declaration, so its identity is a
content digest: a [`Symbol`](../symbols.rs) is the low 128 bits of BLAKE3 over
the name's UTF-8 bytes.

This module is a leaf. It names nothing else in the crate, which is what lets
the [parser](../parse/README.md) and the [type lattice](../type_lattice/README.md)
both rest on it without naming each other.

## Identity is a content digest, the interner is not an authority

`Symbol::of` is a **pure function**. Making a symbol needs no interner, no
registry and no execution context, and equal text yields equal symbols in every
run. The `SymbolInterner` is therefore *not* a lookup authority: comparisons and
probes go straight through symbol bits, and the table is written only where a
syntactic name is constructed and read only where one is rendered. Its growth is
bounded by the run's source text.

That is also why the [type lattice](../type_lattice/README.md) can key its node
table on a digest of the same width and footing with no shared interner between
them, and why the same identity hasher serves both: a digest is already uniformly
distributed, so re-hashing would only cost cycles.

## A symbol carries its binding class

`ValueSymbol`, `TypeSymbol`, `KeywordSymbol` and the `BinderSymbol` that unifies
the two binder classes are distinct types over the same bits, so a field name
arrives already classified by its own parse and no consumer re-derives a class
from text. Equality and digests read the symbol bits alone, so a class rides past
an intern boundary without widening what makes two names the same.

The class itself is a purely lexical rule: a pure-symbol token (no ASCII letters)
is always a keyword, and an alphabetic token is a keyword iff it has at least two
ASCII-uppercase letters and no lowercase ones. A single uppercase letter is
therefore neither a keyword nor a type name — it classifies as neither and is a
parse error.

## Testing

[tests.rs](tests.rs) states the interning laws, beside the four fixed-name pins
[TEST.md](../../TEST.md#symbol-mints) describes.
