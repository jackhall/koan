# Declare-path symbol mints

**Problem.** The declare shape mints 13 symbols per declared name
([`observe/alloc.txt`](../../observe/alloc.txt)'s `declare` sym term). Each unit of the shape
adds five label occurrences (a union tag, a record field, a SIG val, a module LET, an FN
parameter), so the term prices ~2.6 mints per label occurrence — against the label model's own
claim that a name is minted once at the parse that classifies it and every later reader
carries the symbol ([src/machine/model/labels.rs](../../src/machine/model/labels.rs)). Some
declaration-path readers re-hash a spelling the parse already classified — candidates include
the repeated per-slot type-annotation tokens (`:Number` re-classifies at every occurrence),
`LabelInterner::intern` calls on already-classified text, and digest/binder-key construction —
but the mint counter is process-total, so which sites carry the overage is unattributed: a
mint is a BLAKE3 hash, not an allocation, so the dhat profiler cannot see it and no
per-site instrument exists.

**Acceptance criteria.**

- The declare term's per-name mint overage is attributed: each mint site contributing to the
  13/name is named, with its per-name share.
- Every declaration-path read of a label the parse already classified carries the minted
  symbol instead of re-hashing the text.
- The declare sym term in the recorded sweep drops to the attributed floor (one mint per
  distinct classification event), and the figures rebaseline.

**Directions.**

- *Attribution instrument — open.* A `cfg(feature = "alloc-count")` per-site mint tally
  (mirroring the existing process-total `MINTED` in
  [labels.rs](../../src/machine/model/labels.rs)) versus a code-read of the declaration path
  with minimal-shape isolation runs. Recommended: the code-read first — the path is
  parse-static and small — with the per-site tally only if the read leaves residue.
- *Repeated annotation tokens — open.* A type annotation spelling recurs across slots
  (`:Number` on every field), and each occurrence classifies afresh. Whether a parse-scoped
  text→symbol memo pays for itself, or the per-occurrence hash is the correct price of a
  registry-free `Symbol::of`, is a measurement question — decide from the attributed share.

## Dependencies

**Requires:** none — the term is measured and the path is parse-static.

**Unblocks:** none tracked yet.
