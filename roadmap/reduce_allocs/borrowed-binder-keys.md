# Borrowed binder keys

**Problem.** `StoredBinderKey::to_owned_key`
([src/machine/model/binder.rs](../../src/machine/model/binder.rs)) copies each bucket key out
of the parse-static binder plan (`key.to_vec()` per key, plus an intermediate collect and the
outer `Vec`) so the submission path can hand the bindings tables owned map keys — the
`BinderKey` currency `submit` materializes per installing statement
([src/machine/execute/decide/submit.rs](../../src/machine/execute/decide/submit.rs)). The
copies sit on the declare-shape columns of [`observe/alloc.txt`](../../observe/alloc.txt) (a
declaration naming no bucket collects from an empty iterator and allocates nothing, and the
callable-declaring statement count is constant in every shape's `n`, so no marginal term
carries them). The stored plan's key runs are borrows at the node's own lifetime and already
outlive the install; the owned form is pure parameter currency — the function/claim tables
already key on region-bumped `&[KeyElement]` runs
([src/machine/core/bindings.rs](../../src/machine/core/bindings.rs),
[src/machine/core/bindings/claims.rs](../../src/machine/core/bindings/claims.rs)), and the
claim store re-homes a key into its own region only on the first claim of a shape.

**Acceptance criteria.**

- The install path hands the bindings tables the borrowed key run; no per-declaration owned
  copy of a bucket key exists between the stored plan and the table entry.
- The `BinderKey` owned currency (`Vec<UntypedKey>` per installing statement) is gone or no
  longer allocates on the declaration path.
- The declare-shape columns drop by the copies' attributed share and the figures rebaseline;
  the per-name `declare_name` term does not regress.

**Directions.**

- *Table key form — decided.* Borrow the parameter chain
  (`Scope::install_pending_overload` → `Bindings::install_pending_overload` →
  `ClaimStore::claim_bucket`) down to `&[KeyElement]` and stamp the stored plan's runs
  directly; the tables already key on region-bumped runs, and the claim store's existing
  first-claim re-home is the table entry's own storage, so no new lifetime threads through
  the tables and no copy remains. Plan: `scratch/borrowed-binder-keys-plan.md`.
- *Nested-binder rejection read — decided.* The sub-dispatch rejection in `submit` only reads
  emptiness (`key.buckets.is_empty()`); it gates on the stored plan directly and never needs
  the owned form.

## Dependencies

**Requires:** none — the copies are attributed and the table seam is local.

**Unblocks:** none tracked yet.
