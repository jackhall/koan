# Promote untyped invariants into the type system

**Problem.** Runtime invariants across the codebase are enforced by caller
discipline plus runtime panics rather than by the type system. Parser arity
unwraps depend on shape-checks several frames up; `Bindings`/`Scope` coherence
relies on single-writer methods and phase ordering external to the type;
allocator-managed indices are validated by free-list discipline rather than
typed handout; `CallArena`'s `'static` transmute is sound only because
nothing moves its `Rc`'d payload. The survey below catalogs the most
load-bearing of these. Each entry is a candidate for promotion into the type
system whose footprint is local to one module.

**Impact.**

- *Failure modes shift from runtime panic to compile-time error.* Promoted
  invariants are caught by `cargo check`, not by the test suite's coverage of
  unusual paths.
- *Constructor and accessor APIs become self-documenting.* A
  `Pinned<Scope>` returned from `CallArena::new` reads as the contract
  "the scope never moves while this handle is live" without a doc comment.

**Directions.**

- *Collect as independent elements rather than one cohesive design — decided.*
  No element below requires another. The priority list sequences only by
  leverage and blast radius.
- *Per-element type-system mechanism — open.* Each element names the recommended
  shape (newtype, typestate, phantom tag, index newtype) inline alongside its
  trade-offs.

## Elements

### Arena lifetime and heap-pinning discipline

**Where.** [`arena.rs:39-81,281`](../src/runtime/machine/core/arena.rs),
[`kfunction.rs:45-48,116-122`](../src/runtime/machine/core/kfunction.rs),
[`module.rs`](../src/runtime/machine/model/values/module.rs).

`CallArena` heap-pins its scope via `Rc`; `escape_ptr` chains link outer
arenas; the `'static` transmute is sound only because nothing moves the `Rc`'d
payload, a no-move property nowhere encoded. `KFunction<'a>` holds
`NonNull<Scope<'a>>` and relies on caller discipline at `with_pre_run`. Module
child-scope pointer validity rides on the same `Rc` heap-pinning. Promote with
a typed handle whose constructors enforce the pinning contract (e.g. a
`Pinned<Scope>` newtype around the `Rc` with a single `as_ref` API and the raw
pointer never escaping).

### `cycle_close_install_identity` / `register_nominal` phase witness

**Where.** [`scope.rs:248-294`](../src/runtime/machine/core/scope.rs),
[`bindings.rs`](../src/runtime/machine/core/bindings.rs).

Both `cycle_close_install_identity` and `register_nominal` panic on borrow
conflict and on pre-existing `types` entries, with the "post-Combine,
non-re-entrant" phase ordering external to the type. Promote with a phase
witness (e.g. a `PostCombine<'a>` token mintable only by the scheduler)
threaded into the cycle-close path so the panicking branches become
statically unreachable. Lower leverage than the other elements — `RefCell`
enforces borrow contention at runtime regardless of the witness; the marker
primarily documents intent.

### `KFunction::apply` argument-position lookup

**Where.** [`kfunction.rs:417-421`](../src/runtime/machine/core/kfunction.rs).

`pairs.iter().find(|(n, _)| n == &a.name).map(...).expect("missing-arg check
above guarantees presence")` — the `.expect` is sound because a
`pairs.iter().any(|(n, _)| n == name)` walk earlier in the same body confirmed
presence for every signature-side `a.name`. The invariant is presence + name-
to-position mapping, both spelled out in caller-side code rather than in a
type. Promote with a presence-encoded structure built during validation —
either a `HashMap<&str, ExpressionPart>` consumed once per signature element,
or a single sort/reorder pass that lands `pairs` in signature order so the
lookup degenerates into a positional read. Either shape eliminates the
`.expect` because the structure's existence is the presence claim.

### `resolve_literal` quote-placeholder lookup

**Where.** [`expression_tree.rs:21-35`](../src/parse/expression_tree.rs),
[`quotes.rs`](../src/parse/quotes.rs).

`mask_quotes` produces a `HashMap<usize, String>` of placeholder indices and
embeds those indices into the masked string as `QUOTE_PLACEHOLDER`-prefixed
digits. `resolve_literal` later parses the index back out of the masked
string and looks it up in the map — the round-trip from "index minted by
the masker" to "index parsed from a stringly-typed payload" leaks the
allocator-handout shape into untyped input validation. The lookup already
surfaces a structured error on miss (it doesn't panic), so the leverage is
lower than the other elements; the win is encoding "every placeholder in
the masked string was minted by `mask_quotes`" via a typed channel that
sidesteps the `.parse::<usize>()` + `quotes.get` round-trip. Promote with a
`QuoteId` newtype the masker hands out plus a side-channel (or a typed
splice) that carries the resolved literal alongside the masked string, so
the parser never re-derives the index from text.

## Priority

- **`KFunction::apply` argument-position lookup** — small blast radius
  (single function body), moderate leverage (eliminates a `.expect()` and
  collapses an O(n²) lookup); independent of the other elements. Best
  first.
- **`cycle_close_install_identity` / `register_nominal` phase witness** —
  low leverage; `RefCell` already enforces borrow contention at runtime, so
  the witness primarily documents intent. Tractable as a small follow-up
  whenever the surrounding code is being touched.
- **`resolve_literal` quote-placeholder lookup** — marginal leverage (the
  call site already returns a structured error rather than panicking) but
  the smallest blast radius of any element here. Worth taking on whenever
  the lexer/parser interface is being touched for another reason.
- **Arena lifetime and heap-pinning discipline** — highest blast radius,
  deepest into `unsafe`, and load-bearing across the runtime; best taken
  on after the cheaper elements clear the surrounding noise.

## Dependencies

**Requires:** none — each element is local to its named module and can land
independently.

**Unblocks:** none.
