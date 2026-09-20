# The parser

Source text becomes a sequence of `KExpression`s in two passes, and this module
is the second one plus everything the first one's output is spelled in: the
syntax AST and the builtin shape table every node is classified against at
construction, written in the [symbol vocabulary](../symbols/README.md) every
name is minted in.

The [`sexlex`](../../sexlex/README.md) crate reads the text into a layout tree
of atoms, strings, commas and groups and knows nothing about koan. `lower` gives
that tree koan's meaning. `parse` and `parse_with_path` are the entire
text-to-AST surface; `atom`, `brace`, `lower` and `operators` are private.

Outside `#[cfg(test)]` this module reaches [`source`](../source.rs),
[`memory`](../memory/README.md), [`symbols`](../symbols/README.md) and one name
from the [type lattice](../type_lattice/README.md): `KType`, whose builtin
handles are `const` content digests, so a builtin shape states its slots' types
without a registry in hand. That is the whole of the lattice edge — no node, no
registry, no relation — and it runs one way: the lattice rests on
[`symbols`](../symbols/README.md) too, and on nothing here.
A failure is this module's own [`ParseError`](error.rs). The runtime operations
on the types here — lowering a literal, resolving a part to a cell, installing a
binder — are inherent impls in the runtime, which imports them by name.

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
region, so nothing in it has a reach to describe. The scheduler's per-dispatch
form is a distinct type in [`values`](../values/README.md#working-expressions),
and a resolved sub-result or a staging hole lives only there. That separation is what lets the same node be shared across
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
and the builtin shape table. A parts run contributes exactly two things to every
structural question — the bucket key it spells and the class of the head part —
and the cache holds all the answers derived from them: the `ExpressionKey`, the
dispatch shape, the matched builtin shape, and the binder plan.

So every later reader — the dispatch driver, the scheduler's laziness decision,
the close-inference walk, the miss diagnosis — **reads a cached fact rather than
re-walking the run**. Both expression families carry the same cache and answer
these questions the same way, so there is one classifier rather than two.

`DispatchShape` is that classification: bare identifier, bare type leaf, type
call, function-value call, the two sigil wrappers, literal pass-through, operator
chain, the two head-deferred forms, and the general keyworded case. The operator
chain is the one classification no reader past the
[shape builder](../scope/README.md#operator-groups) sees: the builder chains
every operator run into ordinary nodes where a body's shape is built.

## The builtin shape table: one typed entry, every fact

[builtin_shapes.rs](builtin_shapes.rs) holds `BUILTIN_SHAPES`, the one table of
every fixed shape the machine recognizes, spelled once.

A builtin shape is recognized by its **full untyped bucket key**, every keyword
pinned in position — never by a lead keyword. That recognition is sound because
builtin buckets are unshadowable: a node whose key matches a table entry can only
ever resolve to that builtin's overloads, and a key the table marks reserved is
refused to user registration for the same reason.

**An entry is a typed run.** Keywords sit in position, and at each slot a
[`Role`](builtin_shapes/role.rs) sits beside one slot type per overload of the
bucket, with one return per overload. Overloads are columns, not rows: a bucket's
overloads share one keyword run and differ only in the types down each slot, which
is how a user-defined bucket works too — one key, several typed overloads under it
— and which makes it unspellable for two overloads of one bucket to erase to
different keys. What no erasure yields rides the entry beside the run: the binder
it installs and the reserved bit. A node resolves its entry once, at construction,
and every later reader indexes by the `BuiltinShapeId` tag — the close-inference
rules and the miss diagnostics are `(BuiltinShapeId, …)` pairs and hold no key of
their own.

**The untyped facts are erasures of that run**, not columns of their own:

- the **bucket key** a probe compares against is the elements with their types
  dropped, which is what `BuiltinShape::matches` walks;
- the **part kinds a slot keeps raw** are the raw-capture leaves among that slot's
  overload types, which is what `BuiltinShape::lazy_kinds_at` computes: a
  `KExpression` slot keeps an `(…)` group or a `#(…)` quote raw, a
  `SigiledTypeExpr` slot a `:(…)`, a `RecordType` slot a `:{…}`, and a union-typed
  slot keeps each member's kind raw, because it admits every carrier spelling it
  lists.

A slot type rests in the table as a [`KType`](../type_lattice/handle.rs), whose
handle is a `const` content digest, so an entry states its types with no registry
in hand and both erasures are computed at build time. The two compounds a builtin
slot uses — a union of leaves, the empty record — rest as a small recipe instead,
since no `const` computes a compound's digest; interning them is
[`elaborate`](../elaborate/README.md#builtin-shapes)'s.

**Two laws hold the table together at build time**, asserted over the spec as
`const` and so a compile error rather than a test failure:

- every slot of an entry types exactly as many overloads as the entry returns, and
  every bucket has at least one — a slot one type short would leave an overload
  untyped there, and nothing downstream could say which;
- a `Body`, `Branches`, `Quantifiers` or `Data` slot is typed `KExpression` in
  every overload, and an `Rhs` slot keeps nothing raw — a part the machine reads
  as code must reach its reader unevaluated, and a binding's right-hand side is
  classified where it lands.

The table is spelled as a `const` and read through a `static` of the same
contents, because a `const` is what those laws can be evaluated over — a `const`
cannot read a `static`. Every reader takes the `static`, so each
`&'static BuiltinShape` a node caches names one address.

Four readers hang off the table:

- **Roles** ([builtin_shapes/role.rs](builtin_shapes/role.rs)) — what each part of
  an entry is to name resolution: a keyword, a declared name, a right-hand side, a
  body that opens a shape of its own, an arm run, a type declaration's definition,
  data, a label. A part's role decides whether a name in it is a mention at all,
  and how the mention's class moves on the way down (see
  [scope § Visibility](../scope/README.md#visibility)). A body slot also says
  *which kind* of body it opens — a lambda, an operator, a unary operator, a
  `MODULE` or `GROUP` body, or a `USING` body, whose parameters are the names
  its operand
  [surfaces](../scope/README.md#names-that-arrive-at-run-time) rather than
  anything its own form spells. Role is a `BuiltinShape`
  fact, not an `ExpressionShape` one: a user-defined bucket declares no roles.
- **Binder discovery** ([builtin_shapes/binder.rs](builtin_shapes/binder.rs)) —
  pure structural readers plus the facts that ride an entry. A shape is a binder
  *because* its entry carries them, and nothing else declares it. What a binder
  then *does* is the machine's.
- **Raw-capture kinds** ([builtin_shapes/lazy.rs](builtin_shapes/lazy.rs)) — which
  child slots a shape captures raw instead of evaluating. This is a **seal-time**
  fact, not a dispatch-time one: a bare `(…)` evaluates before its parent
  dispatches everywhere except a raw slot of a fixed builtin shape, the node's
  entry is the single source of truth, and the scheduler reads the derivation off
  it to decide child submission. So dispatch selects among overloads over values
  that have already landed, and a reader can tell locally whether a group runs.
  Raw capture is available only to builtin registration — a user `FN` signature
  never receives a raw unquoted group.
- **Slot layout** ([builtin_shapes/layout.rs](builtin_shapes/layout.rs)) — a body's
  value binders as a symbol-sorted run, computed once where the shape is lexically
  fixed and read by every activation of that body, so an activation allocates one
  sized array instead of building a table from nothing. **Slot order is symbol
  order**, never signature or source order: a `FN` and its body agree on a name's
  slot because both resolve it through the same sorted search, with nothing to keep
  in step. The lexical position a binder writes at rides beside each entry, so
  slotting changes the addressing and not the positional visibility rule.

Both the binder facts and the entry's slot types are pinned against the live
builtin registration table by a property test, so an entry whose builtin was
renamed, re-shaped or dropped fails the suite rather than drifting.

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
