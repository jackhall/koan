# Borrowed binder keys

**Problem.** `StoredBinderKey::to_owned_key`
([src/machine/model/binder.rs](../../src/machine/model/binder.rs)) copies each bucket key out
of the parse-static binder plan (`key.to_vec()` per key, plus an intermediate collect and the
outer `Vec`) so the submission path can hand the bindings tables owned map keys — the
`BinderKey` currency `submit` materializes per installing statement
([src/machine/execute/decide/submit.rs](../../src/machine/execute/decide/submit.rs)). The
copies sit on the `declare_name` term of [`observe/alloc.txt`](../../observe/alloc.txt) (a
declaration naming no bucket collects from an empty iterator and allocates nothing, so the
wide per-step path is untouched). The stored plan's key runs are borrows at the node's own
lifetime and already outlive the install; the tables that force the copy are the
function/claim maps keyed by owned `UntypedKey = Vec<KeyElement>`
([src/machine/core/bindings.rs](../../src/machine/core/bindings.rs),
[src/machine/core/bindings/claims.rs](../../src/machine/core/bindings/claims.rs)).

**Acceptance criteria.**

- The install path hands the bindings tables the borrowed key run; no per-declaration owned
  copy of a bucket key exists between the stored plan and the table entry.
- The `BinderKey` owned currency (`Vec<UntypedKey>` per installing statement) is gone or no
  longer allocates on the declaration path.
- The `declare_name` alloc term drops by the copies' attributed share, and the figures
  rebaseline.

**Directions.**

- *Table key form — open.* Key the function/claim tables on the borrowed run at the plan
  lifetime — precedent: the table in
  [bindings.rs](../../src/machine/core/bindings.rs) already keyed on a region-bumped run
  rather than an owned `UntypedKey` — versus keeping owned keys but probing via
  `Borrow<[KeyElement]>` so only a first registration pays a copy. The first deletes the term
  outright but threads a lifetime through the bindings tables; the second is contained but
  leaves one copy per first-registered bucket.
- *Nested-binder rejection read — decided.* The sub-dispatch rejection in `submit` only reads
  emptiness (`key.buckets.is_empty()`); it gates on the stored plan directly and never needs
  the owned form, whatever the table decision.

## Dependencies

**Requires:** none — the copies are attributed and the table seam is local.

**Unblocks:** none tracked yet.
