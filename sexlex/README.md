# sexlex

Whitespace-sensitive s-expressions with glue. This crate is the layout half of a
parser and carries no vocabulary: it turns source text into a tree of atoms,
strings, commas and groups, records which neighbours touched, and stops. Which
atoms are keywords, which glued prefixes are sigils, what a comma means inside a
brace: all of that belongs to the language built on top.

Koan is the first such language; the crate exists so the pattern can be reused
without koan's token classes coming along.

## The pattern

1. **Whitespace separates.** A token is a maximal run of anything that is not
   whitespace, a bracket, a quote or a comma. `:Number`, `a.b`, `->` and `#`
   are single atoms.
2. **Three bracket families group.** `(...)`, `[...]`, `{...}`. A comma is its
   own item.
3. **Quotes delimit strings.** `'...'` and `"..."`, body kept verbatim, a
   backslash escapes the next character, may span lines.
4. **Adjacency is recorded, not interpreted.** Every item says whether the next
   sibling followed it with no whitespace between. A sigil is an atom glued to
   the group after it; the crate never needs to know which atoms are sigils.
5. **Indentation groups lines.** Each non-blank line is a layout group; a
   deeper line nests inside the line above it, a dedent closes. Two-space steps,
   no tabs. Three things suspend the rule: an open `[` or `{` (lines join flat),
   a trailing comma (the next line joins flat), and an open `(` (a deeper line
   nests as its own layout group, the closer may sit on its own line at any
   indentation not less than the opening layout line or at the end of the last
   body line, and a same-or-shallower line before the closer is an error).

The crate doc on `src/lib.rs` is the precise statement of these rules.

## Use

```rust
let items = sexlex::read("LET x =\n  #(3)")?;
assert_eq!(sexlex::render(&items), "L(LET x = L(#~(3)))");
```

`L(...)` is a layout group and `~` marks a glued item. Every `Item` carries a
byte `Span` into the source; every `Error` carries the span that best explains
it (the opener of an unclosed group, the closer that arrived with nothing to
close, the first tab in a bad indentation).

## Tests

`src/tests/examples.rs` pins the rulings a reader consults to learn the rules.
`src/tests/properties.rs` generates random trees, renders them under random
layouts (closer placement, body indentation depth, bracket line breaks, comma
continuations, blank lines) and checks that the tree reads back unchanged, that
every span slices to its own text, and that any unbalancing edit is rejected.
