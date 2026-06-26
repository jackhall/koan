# Borrow-bounded `attach` fallback

Generalize the scope-specialized borrow-bounded `attach` that landed in framestorage into a generic
`Sealed` verb — only if a further call-site migration proves it earns one.

**Problem.** A **scope-specialized** `attach` already exists: the shipped FrameStorage restructure
landed `SealedExtern<ScopeRefFamily>::attach` (a borrow-bounded `&'w Scope<'b>` re-anchor) because
the frame's child-scope readers alloc into the cart region and return the result up-stack, which the
shipped keystone's `open` forbids by construction — its `for<'b>` brand is un-nameable in the result,
so an escaping site has no `open` route. That `attach` is scope-only; the generic
`Sealed<T>::attach<'w>(&'w self, &'w W) -> Live<'w, T>` the substrate would expose for *any* carrier
is not built. Whether the broader migrations — [migrate-reattach-helpers](migrate-reattach-helpers.md),
[value-reads-to-open](value-reads-to-open.md), [scope-reads-to-open](scope-reads-to-open.md) — surface
a non-scope site that also cannot nest, warranting that generic verb (and folding the scope-specific
one into it), is unknown until they run.

**Acceptance criteria.**

- The call-site migrations are surveyed for any re-anchored reference (beyond the frame child scope)
  that must escape the dispatcher call stack and cannot nest under `open`.
- If any such site exists: `Sealed<T, W>` gains a generic borrow-bounded
  `attach<'w>(&'w self, &'w W) -> Live<'w, T>` — re-anchoring capped at the witness borrow, the shape
  of the shipped [`reattach_ref_with`](../../src/witnessed.rs) — with its own Miri tree-borrows proof
  (round-trip, and refuses-when-the-anchor-is-widened); the framestorage scope-specialized `attach`
  folds into it; and the un-nestable site(s) are named with why they cannot fold.
- If no further such site exists: the generic verb is not added, the survey's conclusion is recorded,
  and the framestorage scope-specialized `attach` is the only one — [remove `attach`](remove-attach.md)
  removes it.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *A scope `attach` is already earned; the generic verb stays contingent — decided.* The frame child
  scope demonstrably cannot nest, so its borrow-bounded `attach` shipped in framestorage; whether a
  *generic* `Sealed<T>::attach` is earned beyond it is settled by the remaining migrations.
  Recommended: prefer `open` + copy-out elsewhere and generalize only on a non-scope site that
  demonstrably cannot nest.
- *Borrow-bounded, not free content — decided.* `attach` re-anchors capped at the witness borrow `'w`
  — the witness pin outlives it, a fact the compiler checks — never a free `'b` widenable past the
  pin; that escaping shape is the keystone's rank-2 `open`, not `attach`.

## Dependencies

**Requires:**

- [Migrate the loose witness-borrow wrappers onto `Sealed`](migrate-reattach-helpers.md) — a call-site
  migration this surveys for a non-scope reference that cannot nest under `open`.
- [Migrate result-slot value reads to `open`](value-reads-to-open.md) — surveyed for the same
  un-nestable non-scope reference.
- [Migrate scope-handle reads to `open`](scope-reads-to-open.md) — surveyed for the same.

**Unblocks:**

- [Remove `attach`](remove-attach.md) — removes the `attach`(es) this item leaves, once the read
  migrations clear their consumers.
