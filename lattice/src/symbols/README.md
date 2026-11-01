# Symbols

A symbol — a record field name, a struct schema field, an FN parameter name —
originates in source text and is fixed at declaration, so its identity is a
content digest: a [`Symbol`](../symbols.rs) is the low 128 bits of BLAKE3 over
the name's UTF-8 bytes.

This module is a leaf. It names nothing else in the crate, which is what lets
the [parser](../../../src/parse/README.md) and the [type lattice](../types/README.md)
both rest on it without naming each other.

## Identity is a content digest, the interner is not an authority

`Symbol::of` is a **pure function**. Making a symbol needs no interner, no
registry and no execution context, and equal text yields equal symbols in every
run. The `SymbolInterner` is therefore *not* a lookup authority: comparisons and
probes go straight through symbol bits, and the table is written only where a
syntactic name is constructed and read only where one is rendered. Its growth is
bounded by the run's source text.

That is also why the [type lattice](../types/README.md) can key its node
table on a digest of the same width and footing with no shared interner between
them, and why both hash a digest by its own low word: a digest is already
uniformly distributed, so re-hashing would only cost cycles.

## The load's table and the overlay

The interner keeps its texts in two maps, and may rest on a third, static
tier. The load records every name it constructs in the load's table, which
`freeze` closes at the load's end; a text first recorded after that — a name an
`EVAL` parses at the operator run — lands in the overlay beside it, and a text
the load's table already holds adds nothing there. An interner made
`resting_on` a builtin image's texts — generated with the image, a static run
of slots ([storage](../types/identity.md#storage-a-root-and-its-branches)) —
renders every name the image's types name though no load recorded it, and a
text that tier holds adds nothing to either map. A render and `compare_texts`
read the overlay, the load's table, then the image's tier. A symbol is its
text's digest, so the tiers never disagree: the split decides where a text is
kept, never what a symbol names.

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

## Names fixed in Rust source

Some names are never read out of a program: a keyword a builtin shape fixes, a
builtin type's name, the wildcard `_`. Their spelling is a Rust literal, so
their symbol is the same bits for the whole process. Such a name is **declared
once and compared by symbol thereafter**.

[`static_name!`](../symbols.rs) builds a `StaticName<S>`: the spelling beside a
`LazyLock` memo of its classified symbol, minted through the class's own
`classify`. A memo of a pure function is not run state, since `Symbol::of`
answers the same bits in every run. The class predicate runs once, at first
touch, and a spelling that does not classify panics there, naming itself and
the class it failed. `SymbolInterner::record` reads the memo and records the
spelling, so a diagnostic can still render the name, at the cost of one map
lookup and no hash.

A builtin image's names are fixed by its generator. Each text is a `static`
beside its symbol, and each classified symbol in the image is built from that
static, its class predicate checked when the image compiles. BLAKE3 cannot run
at compile time, so a golden test checks each text's digest.

## Testing

[tests.rs](tests.rs) states the interning laws, including how a `StaticName`
records and where a name recorded after the freeze lands.
