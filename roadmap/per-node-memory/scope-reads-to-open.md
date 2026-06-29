# Fold the scope channel into the step `open`

Open the active scope at the run-loop step brand alongside the continuation, contract, and dep slice,
so the dispatch decide reads `&Scope<'b>` from the one step `open` rather than an escaping re-anchor.

**Problem.** The keystone step `open` ([`run_loop.rs`](../../src/machine/execute/run_loop.rs)) opens
the continuation, return contract, consumer `dest` region, and dep slice together at one rank-2
`for<'b>` brand, so the whole step tail — decide, outcome apply, finalize — runs inside it and consumes
its `Outcome<'b>` in place. The scope is the one carrier not folded in: the decide reads it through the
escaping [`current_scope`](../../src/machine/execute/dispatch/ctx.rs) / `reattach_node_scope` /
[`CallFrame::scope_bounded`](../../src/machine/core/arena.rs), which hand a `&Scope<'step>` up the
dispatcher stack at a free content lifetime — the shape the brand forbids — and `dest` is derived from
an escaping scope read taken *before* the open. A fast lane cannot nest under a *separate* scope `open`
because it returns `Outcome<'step>` and a `for<'b>` closure cannot hand a branded outcome back out; the
resolution is not a per-reader rewrite but folding the scope into the one step brand the tail already
runs in.

**Acceptance criteria.**

- The active scope's carrier is zipped into the step `open` and opened at the brand through the
  consuming `SealedExtern::open`; the dispatch decide receives `&Scope<'b>` from that open.
- No scope read hands a borrow up the dispatcher stack: `current_scope` / `reattach_node_scope` /
  `scope_bounded` (and their escaping `with_*` analogs) are gone or reduced to the brand-threaded read.
- `dest` is the opened scope's `region`, derived inside the brand; the separate `RegionRefFamily` step
  carrier is gone (the region is reached through the scope).
- TCO frame reuse is unaffected — `try_reset_for_tail` keeps its three Miri tests.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Fold the scope into the step `open`, not a per-reader inversion — decided.* The scope is the last
  carrier outside the keystone brand; zip its carrier in, derive `dest` inside, and thread `&Scope<'b>`
  into the decide. A read that provably cannot reach the brand is surfaced here, not retained as a
  borrow-bounded accessor.
- *Open the existing carrier, leave storage to a follow-up — decided.* The frame's child scope already
  rides a `SealedExtern<ScopeRefFamily>` carrier and a node's `YokedChild` an `ErasedScopePtr`; this
  item opens them at the step brand through the consuming `open`, leaving their *storage* representation
  to [scope-pointer-collapse](scope-pointer-collapse.md). What it removes here is the escaping read,
  which clears the borrow-bounded `attach`'s callers.

## Dependencies

Builds on the shipped keystone step `open` (the run-loop tail already nests continuation / contract /
deps at one brand); this folds the remaining channel into that open.

**Requires:** none — the keystone `open` shipped.

**Unblocks:**

- [Collapse the scope-pointer erasure into the substrate](scope-pointer-collapse.md) — opening the
  scope at the brand is what lets a holder's `outer` / `root` re-anchor through its own `open`.
- [`Sealed`: a single access verb](single-open-verb.md) — folding the scope in clears the
  borrow-bounded `attach`'s callers.
