# The parser

Source text becomes a sequence of `KExpression`s in two passes, and this module
is the second one plus everything the first one's output is spelled in: the
syntax AST and the builtin shape table every node is classified against at
construction, written in the [symbol vocabulary](../../lattice/src/symbols/README.md) every
name is minted in.

The [`sexlex`](../../sexlex/README.md) crate reads the text into a layout tree
of atoms, strings, commas and groups and knows nothing about koan. `lower` gives
that tree koan's meaning. `parse` and `parse_with_path` are the entire
text-to-AST surface; `atom`, `brace`, `lower` and `operators` are private.

Outside `#[cfg(test)]` this module reaches [`source`](../source.rs),
[`memory`](../memory/README.md), [`symbols`](../../lattice/src/symbols/README.md) and one name
from the [type lattice](../../lattice/src/types/README.md): `KType`, whose builtin
handles are `const` content digests, so a builtin shape states its slots' types
without a registry in hand. That is the whole of the lattice edge — no node, no
registry, no relation — and it runs one way: the lattice rests on
[`symbols`](../../lattice/src/symbols/README.md) too, and on nothing here.
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

Lowering is where koan's vocabulary enters, and eight rules cover it
([lower.rs](lower.rs)):

- A **run** of sibling items becomes a run of parts, with the context —
  expression, list element, brace entry — deciding where each part lands.
- A **body** is a run that becomes a node. A paren the program writes is kept:
  `((a b))` is `(a b)` wrapped once more, and `#((y))` is no `#(y)`. Only a layout
  line that is a group's whole content comes off, since nobody wrote it as parens.
  A compound atom that classifies to one sub-expression (`a.b`) is that
  sub-expression, not a statement holding it.
- A **sigil** is `#`, `$`, `\` or `:` glued to the group after it. Because
  `sexlex` recorded the glue and kept the atom whole, a sigil needs no table and
  adding one changes nothing below. `$` and `\` are **marks**
  ([quotes](../scope/README.md#holes-and-marks)): glued to a paren, `$(…)` or
  `\(…)` wraps exactly one keyworded use, and one wrapping no keyword, or a
  closed builtin expression shape such as `LET`, is a parse error. Leading an
  atom, a mark marks the name the atom starts with, `$x` or `\x`, so `$a.b` is
  `ATTR $a b`, and an atom that starts with no name (`$3`, `\true`) is a parse
  error naming it. The parser marks wherever it meets a mark; the shape builder
  refuses one no quote value holds. `$..` leads a splice, `$..xs`
  ([splicing](../scope/README.md#splicing)): a part of an expression and
  nothing else, its operand a value name, so `$..(f x)` and `$..3` are errors,
  and so is a splice in a list or brace literal, in a type expression, or in a
  marked use, whose key a splice would leave unknown, and one in a group a
  literal or a [template](#templates) brace holds. `\..xs` has no reading
  and is an error. `#` also glues to
  a `[…]` or `{…}` literal and quotes each element: a paren-group element is
  quoted as that group, any other element as a one-part quote, and a `_` key or
  a record's field name stays bare, so `#{Some: (x), _: (y)}` is
  `{#(Some): #(x), _: #(y)}`. The result is the bare literal — a container of
  quotes, not a quote.
- A **sigil-led line** is a whole layout line whose first atom starts with `#`;
  the line's own body is what it quotes. A mark leads no line, so `$x` alone on
  a line is the marked name. A line that is only a sigil glued to
  its group is that glued sigil. A layout line that is one splice or spread
  atom, `$..xs` or `..xs`, is that atom's part rather than a statement holding
  it, so a splice reads the same in a block laid out on one line or several; a
  written `($..xs)` stays a group.
- **Adjacency** rejects a `[` or `{` glued to a neighbouring token, and a closer
  followed by anything but whitespace, another closer, a `,` or a member read.
  A `.name` glued to a closing paren reads a member of the paren's value, in
  value and type position alike, its suffixes chaining as they do after a word:
  `(MAKESET ints).empty`, `:((MAKESET ints).Set)`. With a space before the dot
  the atom stands alone, and a leading dot is an error.
- A **spread** is `..` glued to a name, `..xs`, or to a paren group,
  `..(f x)`, and is a list literal's item and nothing else: lowering refuses
  `..` anywhere else, inside a `#[…]` literal included, so no other part holds
  one. A spread of a marked name, `..$xs` or `..\xs`, is a list item inside a
  quote that spreads the name's value when the code runs.
- A **template** is `%` glued to a string, `%"…"` or `%'…'`: its text is read
  for braces, and each brace is lowered as a paren group with the same contents
  ([templates](#templates)). A `%` with a space after it is an atom.
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
error, and an empty `{}` is the empty record. No keyword enters a literal but `_`
as a dict's key, which names the dict's default.

### Templates

A template's text is read in [lower.rs](lower.rs), left to right: `{{` and
`}}` are one literal brace each, `{…}` is a **rendered** brace, `%{…}` a
**kept** one, and `%{{` a `%` before a literal `{`. A `}` alone in the text, a
brace with no closing `}`, and a brace holding only whitespace are parse
errors spanning what they name. Text outside the braces is raw, as a string's
body is. A brace closes at the `}` that balances it, counting nested braces and
skipping quoted strings as `sexlex` reads them, and its text is then read on its
own by [`sexlex::read_at`](../../sexlex/README.md), whose spans index the
file, and lowered as a paren group with the same contents is. So a brace holds
one expression: `{x}` is `(x)`, `{a + b}` is `(a + b)`, and a brace whose
expression opens with a brace is spaced off, `{ {a = 1} }`, since `{{` is text.

The part is `Template`, a `KTemplate` of `pieces`, the text between the braces
with each doubled brace read as one, and `slots`, one `TemplateSlot` per brace,
`Rendered` or `Kept`, each holding the brace's group. There is one more piece
than slot. A template with a kept brace evaluates to a record, so lowering
records its field names, `strings` and `values`, as it records any spelling a
program renders. Braces are read only here: no builtin reads a run-time string
as a template, so a brace holds any expression without a string ever becoming
code.

A brace cannot hold the quote character of a template around it, since `sexlex`
ended the string there, so a string or a template inside a brace takes the other
quote, and templates nest at most two deep: `%"{(f %'{x}')}"`. Each brace's read
counts its own groups against `MAX_DEPTH`, and a template's stored depth is one
over its deepest brace's ([the depth limit](#the-syntax-depth-limit)), so syntax
nested through braces is refused as any other syntax is.

Composing a quote stops at a literal: it splices into the groups of the
expression it stands in, and a list, dict or record literal's element or a
template's brace is none of them. So a group such an element or brace holds,
whose node holds a `$..` splice among its own groups, is refused at that group
or brace, while a quote written there composes as its own quote:
`#(%"{#($..p)}")` and `#([#($..p)])` load, as do the quoted elements of a
`#[…]` literal. [Composing inside literals](../../roadmap/metaprogramming/composing-inside-literals.md)
is the work that lets such a splice compose as it does in a paren group.

## The AST: borrowed, `Copy`, and splice-free

A [`KExpression`](ast.rs) node borrows its storage — the parts run and the
structural cache alike — so a node is `Copy`, `Drop`-free, and copying one to
another region is a slice copy rather than a rebuild.

`ExpressionPart` is the vocabulary a run is spelled in: keywords, identifiers and
type names as classified symbols; nested expressions; the two type sigils
(`:(…)` and `:{…}`); list, dict and record literals, a list literal's items
each a `ListItem` — a part, or a spread of one; `Template`, a
[template](#templates)'s pieces and braces; scalar literals;
`QuotedExpression`, the `#(…)` body captured at parse time as data; the two
marked parts, `MarkedName` (`$x`, `\x`), which classifies as the name it marks,
and `MarkedUse` (`$(…)`, `\(…)`), which classifies as a nested expression;
`Splice`, `$..xs`; and `FilledUse`, a keyworded use or operator run whose uses
at the keys it lists a `USING` filled, which classifies as a nested expression.
A `Mark` is `Written` (`$`) or `Built` (`\`) or `Filled`, a hole a `USING`
filled, which only composed code holds and which prints bare. A `Written` or
`Filled` mark carries a **scope**, the quote its binding came from, and whether
it is **pinned**, moved by a read or splice inside a quote value it does not
belong to ([splicing](../scope/README.md#splicing)): 0 and unpinned in
everything the parser writes.

A node holding a `Splice` matches no builtin shape and has no binder plan,
since its arity is unknown until the quote holding it is born. Every node
stores its **weight**, the bytes a copy of its subtree writes, computed at
construction from its parts as its depth is, so a
[code view](../values/README.md#views) prices a stretch of parts without
walking it. A node of more than 16 parts also lays a block memo beside itself
([ast/blocks.rs](ast/blocks.rs)) — running sums of its parts' weights, counts
and keywords every 16 parts, and a sparse table over each block's deepest part
— so the summary of any stretch of its parts reads in O(1).

The copier ([copy.rs](copy.rs)) lays a whole node again through any writer: a
composition's own program storage, or the region a code value's composed
syntax lives in. It may rewrite each mark's scope as it goes, told whether a
quote value lies between the copy's root and the mark before the copy and in
it, and may mark a name filled or hold a use under a filled use; that is how a
code view's materializer and `USING` write scopes, pins and fills.

**Every node carries a source.** Code always comes from somewhere, so a node
holds the [`SourceRef`](../source.rs) — extent and registered file — of the text
it was lowered from, and every construction door takes one. A part's span stays
optional: the parts a brace frame collects keep none, so the sub-expression
wrapping a multi-part key or value is sourced at the whole brace group, and the
one-part quote made for an element of a `#[…]` or `#{…}` literal at the whole
literal. A node built from other code, such as the shape builder's
[operator-run rewrite](../scope/README.md#the-four-rewrites), carries the source
of the code it was built from.

**Quoting is static syntax.** The parser folds the sigil and its group into one
part, so there is no runtime quoting operation and the body never dispatches — a
quote behaves as a literal everywhere.

**Code is taken as a quote.** A callee takes code through a slot typed by a
[code kind](../../lattice/src/types/vocabulary.md#the-code-family), and its caller quotes
it, as a builtin's caller quotes a part that runs later: hygienic fexprs, with
no expansion system and no global execution phase. Rewriting stays the shape
builder's own, as its [pairwise rewrite](../scope/README.md#operator-groups) is,
since a user's rewrite rule would act at a distance.

**A quote is typed by its body as written.** `KExpression::code_kind` reads the
body's [code kind](../../lattice/src/types/vocabulary.md#the-code-family): two or more
statements are a `Block`; a statement of a member-declaring builtin shape a
`Declaration`, and one that installs a `Binder`; a lone scalar literal or nested
quote a `Literal`; a lone name, keyword, `:(…)` or `:{…}` its own kind; and every
other statement an `Expression`. The declaration test is a table fact,
`BuiltinShapeId::declares_member`, asked before the binder plan, since a `VAL`
carries a plan yet installs nothing. A written paren is a part of its
own, so `#((LET x = 1))` is an `Expression`. `ExpressionPart::code_kind` answers
the same for a bare part: a bare group is code of its own kind, as a quote is.

The node here is structurally **splice-free**: an AST node names no producer
region, so nothing in it has a reach to describe. The scheduler's per-dispatch
form is a distinct type in [`values`](../values/README.md#working-expressions),
and a resolved sub-result or a staging hole lives only there. That separation is what lets the same node be shared across
activations without any of them being able to write into it.

### The eternal tier is the lifetime

A node's parts hold plain `&'a KExpression<'a>` references, so a node lies
wholly in the storage its lifetime names. Program storage outlives every
region, so a node at the program's lifetime `'graph` is eternal by that
lifetime alone: a code value holds written syntax at `'graph`, where a copy
shares it and its weight counts nothing past the pointer, and composed syntax
at its region's lifetime, laid there by [code splicing](../scope/README.md#splicing),
where a copy lays it again. For the same reason a string a written literal or
template evaluates to shares the literal's text, and one from composed code
copies it into the region
([lowering](../values/README.md#equality-and-rendering)). A construction door takes a writer at the node's own
lifetime, and the lifetime is invariant at the program brand's doors, so
program storage never holds a node of shorter-lived storage.

### Printing

`ExpressionPart::write_summary` prints syntax so it reads back: a group in its
parens, a string between the quote its body allows (`"` unless the body holds
an unescaped `"`), and every sigil as written. A template prints `%` and its
quote, chosen as a string's is, its pieces with each brace doubled, and each
brace as `{…}` or `%{…}` around its group printed bare, spaced off where it
opens with a brace; a string or a template inside takes the other quote. A scoped mark prints as `$x`,
whatever its scope, a filled mark and a `FilledUse` bare, and a splice as
`$..xs`, so printed code reads back with the holes and marks it was written
with, its bindings and fills aside. A law states it over generated programs:
printing a parsed program and parsing the result gives the same syntax.

## The structural cache: classify once, at construction

A node fills a [`NodeCache`](ast/shape.rs) at construction from its parts run
and the builtin shape table. A parts run contributes exactly two things to every
structural question — the bucket key it spells and the class of the head part —
and the cache holds all the answers derived from them: the `ExpressionKey`, the
dispatch shape, the matched builtin shape, and the binder plan.

So every later reader — the dispatch driver, the shape builder, the
close-inference walk, the miss diagnosis — **reads a cached fact rather than
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

**The bucket key is an erasure of that run**, not a column of its own: the
elements with their types dropped, which is what `BuiltinShape::matches` walks.

**The role says the reading; the type says the syntax.** A slot's role says how
the shape builder reads its part (`Role::reading`), one of five ways: as a written
**quote**, for a part that runs later, conditionally or never — an `EXPR` head
(`Role::Head`), an `OP` symbol or a `PAIRWISE` combiner (`Data`); as **code**,
for a callable's body, which runs later — a written quote, or any other part,
evaluated to code where the callable is born
([building code](../scope/README.md#building-code)); as **bare** syntax, for a
part that declares or runs where it is written, once — a binder name, an in-place `MODULE`, `GROUP` or `USING` body,
`NEWTYPE`'s representation, a type expression (`Role::TypeExpression`), a `TRY`
or `CATCH` operand (`Role::InPlace`); as a **container** of quotes, for a part
that names things as data — an arm set, a union's variants, a `FOR ALL` group, and a `SIG`
body or the heads a bodyless `GROUP` declares (`DefinitionKind::Members`); or
**evaluated**. `ATTR`'s label (`Role::Field`) is the one hybrid: a bare name is
the label itself (`Role::label_reads`), and any other part is evaluated. A body slot's
`BodyKind` says what the builder opens for it (`BodyKind::opens`): a callable's body, a module's
or a block. The slot's type says what
syntax fills it — a code kind, or a container of code kinds, for a part read as
written, and a value type for one evaluated — and no slot keeps a part raw. A
type expression and an in-place operand are the exceptions: they are bare, but
their slot type is the value they denote, so the builder checks their spelling
alone. A callable's body slot is typed `Block`, which a written quote is
admitted against; any other part there is typed by the value it evaluates to,
where the program loads. The table only states those types: the shape
builder's static check ([scope](../scope/README.md#three-tiers)) admits a written part against them
through [`admits_part`](../values/admission.rs) over the program's registry, the
one admission rule, so no rule here restates a container's elements.

A slot type rests in the table as a [`KType`](../../lattice/src/types/handle.rs), whose
handle is a `const` content digest, so an entry states its types with no registry
in hand and its erasure and laws are computed at build time. Every compound a
slot is typed by is a pinned constant of the same kind: the code containers
`List(Name)`, `List(Declaration)`, `Dict(TypeCode, Block)`, `Dict(Name, Block)`
and `Dict(Name, TypeCode)`, the union `TypeCode`, a `FOR ALL` group's union
`List(Name) | Dict(Name, TypeCode)` (`KType::QUANTIFIER_CODE`), and the empty
record. Each is stated once, in the registry's seeding, which every registry
runs; a test asks a fresh registry for every slot type and return, so a table
type no registry seeds fails there.

**Two laws hold the table together at build time**, asserted over the spec as
`const` and so a compile error rather than a test failure:

- every slot of an entry types exactly as many overloads as the entry returns, and
  every bucket has at least one — a slot one type short would leave an overload
  untyped there, and nothing downstream could say which;
- every slot the builder reads as written, save a type expression and an
  in-place operand, is typed by its reading's code type in every overload
  (`roles_agree_with_code_types`): a body `Block`, a head `Expression`, a symbol
  `Keyword`, a label `Name`, an arm set `Dict(TypeCode, Block)` under type
  guards (`MATCH … WITH`) and `Dict(Name, Block)` under labels
  (`MATCH … OVER`, `TRY`), a union's variants `Dict(Name, TypeCode)`, a member
  list `List(Declaration)`, `NEWTYPE`'s representation `TypeCode`, a `FOR ALL` group
  the union `List(Name) | Dict(Name, TypeCode)`, and a binder name a code kind within
  `Expression`; and an `Rhs` slot is `Any`, since a binding's right-hand side is
  classified where it lands.

The table is spelled as a `const` and read through a `static` of the same
contents, because a `const` is what those laws can be evaluated over — a `const`
cannot read a `static`. Every reader takes the `static`, so each
`&'static BuiltinShape` a node caches names one address.

Three readers hang off the table:

- **Roles** ([builtin_shapes/role.rs](builtin_shapes/role.rs)) — what each part of
  an entry is to name resolution: a keyword, a declared name, a right-hand side, a
  head, a body that opens a shape of its own, an arm set, a type declaration's
  definition, data, a field label. A part's role decides how it is read and
  whether a name in it is a mention at all,
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
  *because* its entry carries them, and nothing else declares it. The same
  readers take a definition's head apart into its bucket key, reading each
  slot's label as a name, `_`, or an integer rank — which only a **bucket
  declaration** writes: `EXPR #(MOVE 2 TO 1)`, an entry of its own whose head
  spells a rank or `_` per slot, typed nothing and returning nothing, which
  binds no name and ranks its key
  ([keyworded uses](../scope/README.md#keyworded-uses)). A `NEEDING` list's
  entry reads as a name, or — a one-node quote of keywords and a `_` per slot,
  `#[(LOG _)]` — as a bucket key. What a binder then *does* is the layers
  above's.
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

## The syntax depth limit

Every walk over parsed syntax — lowering, the operator-run rewrite, the shape
builder, a quote's comparison — recurses once per nested part, so how deep a
program's syntax nests is how much stack those walks need. One constant,
`MAX_SYNTAX_DEPTH` ([depth.rs](depth.rs)), bounds it; it is `sexlex`'s
`MAX_DEPTH`, 1024. A program nested past it is refused at load with a parse
error naming the limit, never a crash.

Two checks share the constant, each where the nesting first becomes visible:

- **`sexlex` refuses a group** opened past the limit while it reads, since its
  own descent recurses per group ([sexlex](../../sexlex/README.md#errors)). Every
  group counts, a top-level line's layout group at depth 1, so `PRINT (1)` is 2
  deep. That bounds every paren, bracket and brace a program writes, and so
  every literal.
- **`parse_with_source` refuses a top-level expression** whose stored depth
  passes the limit. The lowered syntax nests where no group does: a dotted
  chain `r.a.a…` is an `ATTR` node per link, and an operator run parses flat
  but is rewritten into nodes nested once per operator.

So every node stores its depth, computed once at construction from its parts'
own stored depths, and no check walks a tree. A node is one level over its
deepest part, where a nested node counts its stored depth, a list, dict or
record literal one more than its deepest element, a template one more than its
deepest brace, and anything else nothing.
Two shapes count more, as the nesting the
[operator-run rewrite](../scope/README.md#the-four-rewrites) builds from them:
a run of `k` operators counts `k + 3` levels, the most any rewrite builds — a
pairwise run's block, its `k - 1` combiners, a pair, the `NOT` of a `!=` pair,
and a statement's wrapper around the block — and a lone `a != b` counts 2, for
`NOT (a == b)`. `PRINT ((1))`, `PRINT [[1]]` and `PRINT r.a.a` are 3 deep, and
`PRINT (1 + 2 + 3)` is 6. The rewrite therefore never deepens a statement past
the depth the parse stored for it, and a run too long for the limit is refused
like deep parentheses, with no cap on run length of its own.

Code composed at run time is held to the same limit where it is composed: a
[code view](../values/README.md#views) stores each node's depth as a node does,
and a splice that would nest past the limit is an error value naming it, so no
walk ever meets syntax deeper than a parse admits.

The limit is sized against a known stack: a host runs a program on a thread of
[`STACK_BYTES`](../program/README.md#the-stack).

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
writes. [depth](tests/depth.rs) pins the depth small shapes store, and a dotted
chain and an operator run refused one level past the limit.
