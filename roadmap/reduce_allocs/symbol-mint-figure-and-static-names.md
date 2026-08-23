# Symbol-mint figure and static names

**Problem.** A `Symbol` is a BLAKE3 digest, and the machine mints one from text on paths
that run per call although the text is fixed in Rust source. `BoundArgs::slot`
([src/machine/core/kfunction/action.rs](../../src/machine/core/kfunction/action.rs)) runs
`Symbol::of(name)` over the builtin body's own `&'static` slot literal on every named argument
read — 46 reads across 17 builtin files, the highest-frequency mint in the tree.
`KType::from_name` ([src/machine/model/types/ktype_resolution.rs](../../src/machine/model/types/ktype_resolution.rs))
matches the 11 builtin type names by string compare at every bind-seam and elaboration fall-through.
The Rust-side tags `Ok` / `Error` / `KError` (`kerror.rs`, `result.rs`, `catch.rs`) are re-minted
per raised error. And nothing measures any of this: hashing is not an allocation, so the
recorded baselines in [audit/README.md](../../audit/README.md) cannot see a mint go away.

**Acceptance criteria.**

- Under the `alloc-count` feature the binary prints `symbols_minted: N` beside `allocations: N`,
  counting every `Symbol::of` / `Symbol::of_parts` mint; `audit/measure.sh` captures it and the
  [audit/README.md](../../audit/README.md) table carries it as a column for every shape.
- A fixed Rust-side name is declared once as a `StaticName<S>` — its spelling beside a lazily
  minted classified symbol — and every consumer compares that symbol: a builtin slot name is
  declared once and used by both its registration (`arg(registries, &NAME, …)`, which interns the
  spelling) and its body reads (`args.object(&NAME)`); `BoundArgs::slot` takes a symbol and mints
  nothing; `KType::from_symbol(TypeSymbol)` keys the builtin type table on static `TypeSymbol`s;
  the `Ok` / `Error` / `KError` tags are statics.
- The `symbols_minted` figure drops on every shape and the per-call terms of the builtin-call and
  user-FN shapes quote the drop; the recorded allocation baselines do not regress.

**Directions.**

- *Static form — decided.* `StaticName<S> { text: &'static str, symbol: LazyLock<S> }` in
  `labels.rs`, minted by a `static_symbol!` macro; one declaration per name. A `LazyLock` over a
  pure function of a literal is a memo, not run state. Rejected: positional slot reads (sound today
  — every builtin body serves one layout — but an index constant tracks registration order by hand
  and a reorder reads the wrong slot silently, where a name miss is a `MissingArg`) and pinned
  hardcoded digests (the `KType` const pattern; maintained by a pin test rather than by
  construction).
- *Figure bounded or recorded — decided.* Recorded in the table and quoted in commit messages;
  not a regression test (the integration test cannot reach a lib-side feature-gated counter).

## Dependencies

**Requires:** none — the instrument the rest of the symbol-only program is measured by.

**Unblocks:**

- [Parse-interned type tokens](parse-interned-type-tokens.md) — measured by the figure; retires
  `from_symbol`'s `&str` adapter.
- [Symbol-only keyword tokens](symbol-only-keyword-tokens.md) — static keyword names.
