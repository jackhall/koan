# Parse a token without building a buffer for it

**Problem.** The parse copies each token's text out of the source twice, once to accumulate
it and once to split it, and the two copies are the largest attributed share of the
`declare_name` term — 23 of its 57 allocations per declared name, measured by a dhat
difference of `audit/shapes/declare_n{10,100}.koan`.

The outer copy is the tokenizer's accumulator.
[`build_tree`](../../src/parse/expression_tree.rs) pushes each consumed codepoint onto a
`String` and hands it to [`flush_token`](../../src/parse/parse_stack.rs), which takes the
buffer with `std::mem::take` — so the heap allocation the last token grew is dropped with it
and the next token starts from empty. The accumulator never keeps a capacity, and the growth
costs 13 allocations per declared name.

The inner copy is the atom reader. [`read_atom`](../../src/parse/tokens.rs) walks the token's
characters to the next atom terminator and pushes each onto a fresh `String`, purely to hand
`&s` to `classify_atom`. Nothing is transformed on the way — an atom is a contiguous verbatim
run of the token, and the reader already computes the byte offsets at both ends — so the
`String` exists only because the reader is handed a `Peekable<CharIndices>` and not the text
those indices address. That is 10 allocations per declared name, one per atom in every parse
in the repo.

**Acceptance criteria.**

- The tokenizer's accumulator keeps its capacity across tokens: classifying one borrows the
  buffer and empties it, so a program's token stream grows a buffer as large as its longest
  token and no larger.
- `read_atom` classifies from a borrowed slice of the token it is reading, and allocates
  nothing per atom.
- A dhat difference of `audit/shapes/declare_n{10,100}.koan` attributes no allocation to
  either site, and the `declare_name` term in [`observe/alloc.txt`](../../observe/alloc.txt)
  falls by their share.
- Parse diagnostics quote the same spans and spellings they do today — every error a malformed
  atom or an unterminated token raises is unchanged.

**Directions.**

- *How the atom reader reaches its text — open.* Thread the token `&str` alongside the
  `Peekable<CharIndices>` and slice it by the offsets the walk already computes, or replace the
  iterator pair with a cursor type that owns both. Recommended: the extra `&str` parameter,
  which is local to `parse_compound` and its two `read_atom` callers.
- *Whether the accumulator can become a source slice outright — open.* Most tokens are a
  verbatim run of the source, but not all: `build_tree` injects a `>` into the buffer on one
  arm, so a slice-only accumulator needs somewhere for a synthesized token to live. Emptying
  the buffer rather than taking it is the close that does not depend on settling this.

## Dependencies

**Requires:** none — foundation.

**Unblocks:** none tracked yet.
