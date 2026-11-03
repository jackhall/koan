# A program owns its source

**Problem.** Every load registers its text in a thread-wide registry.
[`parse_with_path`](../../src/parse.rs) copies the program text into a
`SourceFile`, with its line table, and pushes it onto the thread-local
`SOURCES` vector ([source.rs](../../src/source.rs)); a `FileId` is that
entry's index, and a `SourceRef` resolves its text and its line through it.
The registry is append-only, so that every `FileId` resolves, and nothing ever
frees an entry: a thread that loads programs again and again — a REPL, the
test binary, an embedder — keeps every text it has loaded, about 1.8 KB per
load of a 1.5 KB program. It is run-wide mutable state, which AGENTS.md rules
out: a `SourceRef` resolves only on the thread that registered it, and the
file parse diagnostics are attributed to, `CURRENT_FILE`, is a second
thread-local beside it.

**Acceptance criteria.**

- A loaded program owns its source text and line table, and both are freed
  when the program is dropped.
- A `SourceRef` resolves its text, line and column through the program that
  holds it, and `src/source.rs` holds no thread-local.
- Loading and dropping the same program a thousand times on one thread leaves
  the thread's live heap where loading and dropping it once leaves it.

**Directions.**

- *Where a source lives — open.* Either the program's storage owns its
  `SourceFile`s and a `FileId` indexes the program's own table, or the
  substrate owns a registry beside the program. A `SourceRef` stays `Copy`
  and drop-free either way, since spans rest in region-laid data.
  Recommended: the program's storage, beside the parse that indexes it.
- *How a diagnostic reaches the text — open.* `SourceRef::text` and parse
  errors read the registry with no context today. Either each rendering path
  is handed the program's sources, or a parse error resolves its line and
  column when it is raised.

## Dependencies

[A refused program stays loaded](refused-programs-stay-loaded.md) keeps a
refused program's parse, whose error renders through the same sources.

**Requires:** none.

**Unblocks:** none — a leaf.
