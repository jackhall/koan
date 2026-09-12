# sexlex

*(working name)*

Whitespace-sensitive s-expressions with glue: the layout half of a parser, with
no vocabulary of its own. [`read`](src/lib.rs) turns source text into a tree of
atoms, strings, commas and groups, records which neighbours touched, and stops.
Which atoms are keywords, which glued prefixes are sigils, what a comma means
inside a brace — all of that belongs to the language built on top.

Koan is the first such language ([README.md](../README.md)); the crate exists so
the pattern can be reused without koan's token classes coming along. Nothing in
here names a koan type, and nothing in here needs to: the whole crate is a
statement about *shape*.

## What the crate decides, and what it refuses to

A parser for an indentation-sensitive language has two jobs that are usually
tangled together: working out what nests inside what, and working out what the
nested things mean. Tangling them costs twice — the layout rules end up restated
once per construct, and a language wanting the same layout with different
vocabulary has to fork the parser.

sexlex is the first job alone. It knows exactly five things, stated precisely by
the crate doc on [src/lib.rs](src/lib.rs):

1. **Whitespace separates.** A token is a maximal run of anything that is not
   whitespace, a bracket, a quote or a comma.
2. **Three bracket families group.** `(...)`, `[...]` and `{...}` become
   `Paren` / `Bracket` / `Brace` groups; a comma is its own item.
3. **Quotes delimit strings.** `'...'` and `"..."`, body kept verbatim, a
   backslash escapes the next character, may span lines.
4. **Adjacency is recorded, not interpreted.**
5. **Indentation groups lines**, with three regimes that suspend the rule.

Three refusals follow from that, and each one is load-bearing:

- **No token classes.** `:Number`, `a.b`, `->`, `#` and `x` are all one atom,
  because the break set is punctuation-and-space and nothing else. A language
  that wants `.` to be an operator splits the atom itself, on its own terms,
  and a language that wants `a.b` to be one name does nothing. The crate never
  has to be told which spellings are special.
- **No sigil table.** Rule 4 records *whether* two items touched; it never asks
  why. A sigil is simply an atom glued to the group after it, so `#(3)` reads as
  an atom and a paren group with a glue bit set between them, and the layer above
  decides that `#` quotes. Adding a sigil to koan is not a change here.
- **No expression shape.** The output is groups and leaves. There is no
  precedence, no application, no arity — a `Paren` group is a bracket pair that
  closed, not a call.

The payoff is that the rules compose: every construct in the language above gets
the same line-joining, the same glue, the same bracket nesting, for free, and
there is one place to read what layout means.

## The tree

Three types carry the whole output ([src/lib.rs](src/lib.rs)):

- **`Item`** — a `Node`, the `Span` it covers, and `glued`: whether the next
  sibling followed with no whitespace between. Adjacency lives on the *left* of
  the pair so a reader walking a run of items decides about the gap it has just
  passed, without lookahead.
- **`Node`** — `Atom(&str)`, `Str { quote, body }`, `Comma`, or
  `Group { kind, items }`.
- **`Kind`** — `Layout`, `Paren`, `Bracket`, `Brace`. The three bracketed kinds
  come from delimiters in the text; `Layout` is synthesized from line structure,
  which is why [`Kind::delimiters`](src/lib.rs) returns `None` for it.

The tree **borrows the source it was read from**: an atom is a `&'s str` into
the input, a string body is the bytes between the quotes untouched, and every
span is a byte range into that same text. So reading allocates the tree and
nothing else, and the language above can hold the borrow for as long as it holds
the source. Escape processing is deliberately *not* done here — `body` is raw,
because what `\n` means is vocabulary.

A group's span covers its whole extent: a bracketed group from opener through
closer, a string across both quotes, a layout group from its first item's start
to its last item's end. `glued` is always `false` on a layout group, on the item
before one, and on a group's last item, because a line break or a delimiter
intervenes in each case.

## Reading, in two passes

[`lex`](src/lex.rs) flattens the source to a token stream, then
[`reader`](src/reader.rs) descends it into the tree. The split is what keeps the
indentation rules readable: the lexer settles what the *characters* say and the
reader settles what the *lines* mean, and neither has to reason about the other.

**The lexer** emits one `Token::Line` per non-blank line carrying that line's
indentation (and the position of the first tab in it, if any), then one token per
item, each recording whether whitespace preceded it. Blank lines produce nothing
at all, so a blank line inside a block never ends it. Two decisions live here:

- The lexer *measures* indentation but does not *judge* it. A tab is reported as
  a span and an odd width is passed through, because whether either matters
  depends on the position the line lands in — a continuation line joining the
  line above it has free-form indentation, and rejecting its tabs would be wrong.
  The reader, which knows the position, does the judging.
- `spaced` is recorded per token and turned into `Item::glued` by the reader,
  which reads it from the *following* token. That inversion is why a `Vec` of
  tokens and not a streaming lexer: the glue bit of an item is a fact about the
  item after it.

**The reader** is recursive descent with two pieces of state: `layout_indent`,
the indentation of the layout line currently being read, and `flat`, the depth of
enclosing `[` / `{`. Everything about the three regimes falls out of those two.

## The three regimes

Rule 5's default is simple: a non-blank line is a `Layout` group, and every line
indented deeper nests inside it. Three things suspend it, and each exists for a
concrete shape the language above wants.

**Inside `[` or `{`, lines join flat.** `flat` goes above zero and every `Line`
token is swallowed. A list or a record laid out over several lines is one run of
items, which is what a reader of a collection wants — the line breaks were
formatting.

**A line ending in `,` joins the next line flat**, whatever its indentation. This
is the continuation escape hatch outside brackets: an argument run that grew too
long breaks after a comma and resumes, and the resumed text is a sibling rather
than a child. It is checked on the item just read, so it composes with everything
else without a flag.

**Inside `(` at flat depth zero, layout still applies**, and this is the subtle
one. A deeper line nests as its own layout group, one per line, so a parenthesized
expression can carry a block. The `(` anchors to `layout_indent` — the line it was
opened on, not its own column — which is what makes a `(` on a comma-joined
continuation measure against the line the continuation joined. Two errors guard
the regime:

- a line at or above the anchor arriving before the `)` is a `DanglingParen`: the
  expression broke before the paren closed, which is almost always a missing `)`
  rather than an intended shape;
- a `)` on its own line less indented than the anchor is a `CloserDedented`.

Where the closer sits is layout, not structure: it may end the last body line or
sit on its own line at any indentation not less than the anchor, and either reads
the same. Text after it continues the opening layout line, which is why the reader
restores `layout_indent` after a nested block.

A closer is owned by the innermost *bracketed* group and never by a layout group.
So a `)` met inside a layout line ends that line, and every layout group above it
up to the paren, and is left unconsumed for the paren to take. That one rule is
what lets a block close without counting dedents.

## Errors

Every [`ErrorKind`](src/lib.rs) carries the span that best *explains* it, which
is not always the span where the reader noticed. An unclosed group points at its
opener, not at end-of-input; a mismatched closer points at the closer but carries
the opener's span too; a dangling paren points at the `(`. A structural error's
useful location is the place the author has to edit, and finding it is the
reader's job rather than the caller's.

The set is small and total: tab indentation, odd indentation, unclosed string,
unclosed group, unexpected closer, mismatched closer, dangling paren, dedented
closer. There is no "unexpected token" case, because there is no token the crate
does not accept — only structures that do not close.

## Rendering

[`render`](src/render.rs) prints a tree on one line: layout groups as `L(...)`,
bracketed groups with their own delimiters, strings with their quotes, and `~`
instead of a space after a glued item.

```rust
let items = sexlex::read("LET x =\n  #(3)")?;
assert_eq!(sexlex::render(&items), "L(LET x = L(#~(3)))");
```

It is lossless for structure and glue while discarding layout entirely, which is
exactly the equivalence the crate claims — so it is both the diagnostic format
and the comparison the property suite is written against.

## Testing

The rules are pinned two ways, in [src/tests](src/tests.rs):

- **`examples`** holds the rulings a reader consults to learn the rules: one
  input, one rendered tree or one error message, per rule and per corner.
- **`properties`** generates random trees, renders them back out under *random
  layouts* — closer placement, body indentation depth, bracket line breaks, comma
  continuations, blank lines — and checks three laws: the tree reads back
  unchanged (layout never changes structure), every span slices to its own text,
  and any unbalancing edit is rejected.

The first law is the crate's whole contract stated as a property, and the third
is what makes the error set's totality checkable rather than asserted.
