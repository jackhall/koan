# Yoke the program AST into the run region

Make the parsed program region-resident and carrier-exposed, so every value that embeds an AST
reference is built by `yoke` / `merge` — never an asserted `Witnessed::new`.

**Problem.** [`run_program`](../../src/machine/execute/runtime/interpret.rs) borrows
[`parse`](../../src/parse.rs)'s owned `Vec<KExpression>` at `'run`; every FN body, quoted expression,
and signature is a bare `&'run` / `&'ast` AST reference embedded in the value a construction builds. The
`for<'b>` brand on [`Witnessed::yoke`](../../src/witnessed.rs) cannot capture a non-`'static` borrow,
and a bare AST reference is no carrier, so each AST-embedding construction (`alloc_function`, `quote`,
the FN signature) can only bundle its value with an asserted witness via
[`Witnessed::new`](../../src/witnessed.rs) — the structurally-asserted co-location form, not the
compile-enforced `yoke` / `merge`. `Witnessed::new` is the substrate's one asserted constructor; while
the AST stays a bare borrow it cannot be retired.

**Acceptance criteria.**

- The parsed program is yoked into the run region at `run_program`: `parse`'s owned `Vec<KExpression>`
  moves into a `yoke` closure (the only origin the `for<'b>` brand admits — a pre-existing `&'run` ref
  cannot be captured), and the program is region-resident thereafter, carried by the run frame.
- Every FN body, quoted expression, and signature flows as a run-region carrier (or a `&'run` ref a
  construction `merge`s), not a bare borrow the construction must `new`.
- `alloc_function` and `quote` build their value by `merge`-ing the body / expression carrier into it,
  carrying no `Witnessed::new`.
- The full Miri slate is green — the run region frees the AST at run end, so there is no leak; `cargo
  test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Yoke from the owned parse output — decided.* The carrier originates from `parse`'s owned
  `Vec<KExpression>` inside a `yoke` closure at the run boundary; the brand cannot capture a pre-existing
  `&'run` ref, so a per-FN-def yoke is impossible — the program is region-ified once, at `run_program`.
- *Run-region hosting, not a `'static` leak — decided.* Hosting in the run region frees the AST at run
  end; leaking it to `'static` never frees it (unbounded for a library host re-parsing programs, and
  Miri's leak detector flags it at process exit).
- *The keystone for zero structural-`new` — decided.* Once the AST is a carrier, every AST-embedding
  construction `merge`s it; this is what lets [alloc-object](alloc-object-witnessed.md) drop the
  `Witnessed::new` its `alloc_function` / `quote` inversions would otherwise assert.
- *Sub-expression carrier projection — open.* How a single FN body / quoted sub-expression is projected
  out of the whole-program carrier (a `map` down the program carrier vs. a finer-grained per-statement
  yoke) is an implementation choice to settle against the dispatch path. Recommended: project via `map`
  so the program is yoked once.

## Dependencies

**Requires:** none — foundation (the witness substrate is shipped).

**Unblocks:**

- [`alloc_object` returns `Witnessed`](alloc-object-witnessed.md) — its `alloc_function` / `quote`
  inversions `merge` the AST carrier this lands, rather than asserting co-location via `Witnessed::new`.
