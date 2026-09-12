# The parser

Source text becomes a sequence of `KExpression`s in two passes, and this module
is the second one plus everything the first one's output is spelled in: the
label vocabulary every symbol is minted in, the syntax AST, and the form table
every node is classified against at construction.

The [`sexlex`](../../sexlex/README.md) crate reads the text into a layout tree
of atoms, strings, commas and groups and knows nothing about koan. `lower` gives
that tree koan's meaning. `parse` and `parse_with_path` are the entire
text-to-AST surface; `atom`, `brace`, `lower` and `operators` are private.

Outside `#[cfg(test)]` this module reaches only [`source`](../source.rs) and
[`memory`](../memory/README.md); a failure is its own
[`ParseError`](error.rs). The runtime operations on the types here — lowering a
literal, resolving a part to a cell, installing a binder — are inherent impls in
the runtime, which imports them by name.

## The division of labour with `sexlex`

The split is the design decision the rest of the module rests on: **layout is
decided without vocabulary, and vocabulary is applied without re-deciding
layout.**

`sexlex` settles what nests inside what — whitespace separation, three bracket
families, quoted strings, adjacency recorded as a glue bit, and indentation with
its three suspending regimes. It never asks which atoms are keywords or which
glued prefixes are sigils.

Lowering is where koan's vocabulary enters, and six rules cover it
([lower.rs](lower.rs)):

- A **run** of sibling items becomes a run of parts, with the context —
  expression, list element, brace entry — deciding where each part lands.
- A **body** is a run that becomes a node, and is where a redundant wrapper is
  peeled: `((a b))` and `(a b)` name the same expression, and so does the group a
  body line nests in.
- A **sigil** is `#`, `$` or `:` glued to the group after it. Because `sexlex`
  recorded the glue and kept the atom whole, a sigil needs no table and adding
  one changes nothing below.
- A **sigil-led line** is a whole layout line whose first atom starts with `#` or
  `$`; the line's own body is what it quotes.
- **Adjacency** rejects a `[` or `{` glued to a neighbouring token.
- Everything else is an **atom**, which [atom.rs](atom.rs) classifies.

Atom classification is the one place text is read closely: literals; a split on
colons into a word and the type names annotating it (`x:Number`); the
keyword / type / identifier classification; and the compound desugarings (`a.b`,
`a?`) driven by the small [operators](operators.rs) table. A pure-symbol token
that is not a builtin compound trigger tags as a `Keyword`, which is what lets a
post-parse chain detector recognize a user-declared operator without the lexer
knowing it exists.

Brace literals get their own sub-state-machine ([brace.rs](brace.rs)) because
one `{…}` frame serves two containers: a **dict** (`{k: v}`) and a **record**
(`{x = 1}`). The first pairing operator selects the mode, mixing the two is an
error, and an empty `{}` is the empty record.

## Labels: identity is a content digest, the interner is not an authority

A label — a record field name, a struct schema field, an FN parameter name —
originates in source text and is fixed at declaration, so its identity is a
content digest: a [`Symbol`](labels.rs) is the low 128 bits of BLAKE3 over the
label's UTF-8 bytes.

`Symbol::of` is a **pure function**. Making a symbol needs no interner, no
registry and no execution context, and equal text yields equal symbols in every
run. The `LabelInterner` is therefore *not* a lookup authority: comparisons and
probes go straight through symbol bits, and the table is written only where a
syntactic label is constructed and read only where one is rendered. Its growth is
bounded by the run's source text.

That is also why the [type lattice](../type_lattice/README.md) can key its node
table on a digest of the same width and footing with no shared interner between
them, and why the same identity hasher serves both: a digest is already uniformly
distributed, so re-hashing would only cost cycles.

**A symbol carries its binding class.** `ValueSymbol`, `TypeSymbol`,
`KeywordSymbol` and the `BinderSymbol` that unifies the two binder classes are
distinct types over the same bits, so a field name arrives already classified by
its own parse and no consumer re-derives a class from text. Equality and digests
read the symbol bits alone, so a class rides past an intern boundary without
widening what makes two labels the same.

The class itself is a purely lexical rule: a pure-symbol token (no ASCII letters)
is always a keyword, and an alphabetic token is a keyword iff it has at least two
ASCII-uppercase letters and no lowercase ones. A single uppercase letter is
therefore neither a keyword nor a type name — it classifies as neither and is a
parse error.

## The AST: borrowed, `Copy`, and splice-free

A [`KExpression`](ast.rs) node borrows its storage — the parts run and the
structural cache alike — so a node is `Copy`, `Drop`-free, and copying one to
another region is a slice copy rather than a rebuild.

`ExpressionPart` is the vocabulary a run is spelled in: keywords, identifiers and
type names as classified symbols; nested expressions; the two type sigils
(`:(…)` and `:{…}`); list, dict and record literals; scalar literals; and
`QuotedExpression`, the `#(…)` body captured at parse time as data.

**Quoting is static syntax.** The parser folds the sigil and its group into one
part, so there is no runtime quoting operation and the body never dispatches — a
quote behaves as a literal everywhere.

The node here is structurally **splice-free**: an AST node names no producer
region, so nothing in it has a reach to describe. The scheduler's per-call form
is a distinct type in the runtime, and a resolved sub-result or a staging hole
lives only there. That separation is what lets the same node be shared across
activations without any of them being able to write into it.

### The eternal tier is a type, not a discipline

[ast/program.rs](ast/program.rs) holds `ProgramExpression` and `ProgramNode`,
two `Copy` newtypes whose private fields make "this node's parts run is hosted in
program storage" a property the compiler checks.

The claim is about the **parts slice**, not the node struct: a `KExpression` is
`Copy` and rides by value, so what a holder can outlive is the run of parts the
node borrows and everything reachable from it. That is why the marker sits on the
references *inside* the expression-holding part arms rather than on the node, and
why re-homing the node struct itself at any brand is sound. The marker is
consumed only where the claim is used — the dispatch channel keeps carrying bare
`KExpression` — so nothing goes viral and there is no erase point to audit.

## The structural cache: classify once, at construction

A node fills a [`NodeCache`](ast/shape.rs) at construction from its parts run
and the form table. A parts run contributes exactly two things to every
structural question — the bucket key it spells and the class of the head part —
and the cache holds all the answers derived from them: the untyped key, the
dispatch shape, the operator probe, the matched builtin form, and the binder
plan.

So every later reader — the dispatch driver, the scheduler's laziness decision,
the close-inference walk, the miss diagnosis — **reads a cached fact rather than
re-walking the run**. Both expression families carry the same cache and answer
these questions the same way, so there is one classifier rather than two.

`DispatchShape` is that classification: bare identifier, bare type leaf, type
call, function-value call, the two sigil wrappers, literal pass-through, operator
chain, the two head-deferred forms, and the general keyworded case.

## The form table: one entry, every fact

[forms.rs](forms.rs) holds `FORMS`, the one table of every fixed form the machine
recognizes, spelled once.

A builtin form is recognized by its **full untyped bucket key**, every keyword
pinned in position — never by a lead keyword. That recognition is sound because
builtin buckets are unshadowable: a node whose key matches a table entry can only
ever resolve to that builtin's overloads, and a key the table marks reserved is
refused to user registration for the same reason.

Each entry carries every fact the machine reads off a form under one `FormId`
tag: the binder it installs, the slots that stay raw, whether the shape is
reserved. A node resolves its entry once, at construction, and every later reader
indexes by the tag — the close-inference rules and the miss diagnostics are
`(FormId, …)` pairs and hold no key of their own.

Three readers hang off the table:

- **Binder discovery** ([forms/binder.rs](forms/binder.rs)) — pure structural
  readers plus the facts that ride an entry. A form is a binder *because* its
  entry carries them, and nothing else declares it. What a binder then *does*
  is the machine's.
- **Lazy slots** ([forms/lazy.rs](forms/lazy.rs)) — which child slots a form
  captures raw instead of evaluating. This is a **seal-time** fact, not a
  dispatch-time one: a bare `(…)` evaluates before its parent dispatches
  everywhere except a lazy slot of a fixed builtin form, the node's entry is the
  single source of truth, and the scheduler reads it to decide child submission.
  So dispatch selects among overloads over values that have already landed, and a
  reader can tell locally whether a group runs. Lazy declaration is available only
  to builtin registration — a user `FN` signature never receives a raw unquoted
  group.
- **Slot layout** ([forms/layout.rs](forms/layout.rs)) — a body's value binders
  as a symbol-sorted run, computed once where the shape is lexically fixed and
  read by every activation of that body, so an activation allocates one sized
  array instead of building a table from nothing. **Slot order is symbol order**,
  never signature or source order: a `FN` and its body agree on a name's slot
  because both resolve it through the same sorted search, with nothing to keep in
  step. The lexical position a binder writes at rides beside each entry, so
  slotting changes the addressing and not the positional visibility rule.

Both the binder facts and the lazy-slot kinds are pinned against the live builtin
registration table by a property test, so an entry whose builtin was renamed,
re-shaped or dropped fails the suite rather than drifting.

## Errors

A [`ParseError`](error.rs) is a message, the span it was observed at, and the
file the span indexes. The parser reaches nothing above `source`, so its failure
is its own leaf type rather than the runtime's error; the runtime wraps one whole
and renders it through this file's `Display`, so a parse error reads the same
from the CLI, from a test asserting on the message, and from the value a program
catches.

Spans are carried so diagnostics point at the right character: a synthetic
operator keyword takes a one-codepoint trigger span, while a mid-token error
attaches the enclosing token's span, so the message names the offending character
while the span pinpoints the token.

## Testing

The suite ([tests.rs](tests.rs)) renders an `ExpressionPart` tree as a compact
`t(...)` / `T(...)` notation and asserts on that string, so an expectation reads
as the shape it names.

`properties` states the parser's **laws** over random trees rendered under random
layouts, in that same notation. The files beside it hold what a law does not
state: the diagnostic a mistake reports, and the surface rules a renderer never
writes.

## A note on `pending_rewrite`

The runtime is this module's consumer, and the runtime is behind the
`pending_rewrite` feature. An item marked
`cfg_attr(not(feature = "pending_rewrite"), allow(dead_code))` — or an
`unused_imports` twin on a crate-visible re-export — has no caller in a default
build until the rewrite adopts it, and the marker comes off with the adoption.
