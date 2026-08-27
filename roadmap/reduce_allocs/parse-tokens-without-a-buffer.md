# Parse a token without building a buffer for it

**Problem.** The parse copies each token's text out of the source twice, once to accumulate
it and once to split it, and the two copies are the largest attributed share of the
`declare_name` term — 25 of its 55 allocations per declared name, measured by a dhat
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
those indices address. That is 12 allocations per declared name, one per atom in every parse
in the repo.

**Acceptance criteria.**

- Tokenization builds no accumulator at all: a token is classified as a borrowed slice of
  the masked byte stream, so no per-token or per-program buffer exists to grow.
- `read_atom` classifies from a borrowed slice of the token it is reading, and allocates
  nothing per atom.
- A dhat difference of `audit/shapes/declare_n{10,100}.koan` attributes no allocation to
  either site, and the `declare_name` term in [`observe/alloc.txt`](../../observe/alloc.txt)
  falls by their share.
- Parse diagnostics quote the same spans and spellings they do today — every error a malformed
  atom or an unterminated token raises is unchanged.

**Directions.**

- *How the atom reader reaches its text — decided.* Thread the token `&str` alongside the
  `Peekable<CharIndices>` and slice it by the offsets the walk already computes; the extra
  parameter is local to `parse_compound` and its two `read_atom` callers.
- *The accumulator becomes a slice of the masked stream — decided,* per
  `scratch/parse-tokens-without-a-buffer-plan.md`. Every token is a contiguous byte run of
  the masked stream: the `>` that `build_tree` glues onto a pending `-` only glues when the
  `>` is the next masked byte, and every JUMP marker sits at a token boundary (collapse
  plants them only before its synthetic `(` / `)` / space / sigil bytes, and the
  post-literal JUMP is consumed inside the quote arm, which flushes first). So the pending
  token is a `(masked_start, masked_end, span_start)` record — the end tracked at
  accumulation time, not read from the reader at flush — and `flush_token` classifies
  `&masked[start..end]` directly.

## Dependencies

**Requires:** none — foundation.

**Unblocks:** none tracked yet.
