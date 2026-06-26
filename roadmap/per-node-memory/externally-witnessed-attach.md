# Borrow-bounded `attach` fallback

Add the borrow-bounded `attach` accessor over the externally-witnessed sealed form — only if a
call-site migration proves it needs a re-anchored reference to escape up-stack where the keystone's
`open` cannot nest.

**Problem.** The [keystone](runloop-cps-open.md) adds the consuming externally-witnessed `open` and
proves the run-loop step tail nests under it with no reference escaping the dispatcher call stack.
The broader call-site migrations — [migrate-reattach-helpers](migrate-reattach-helpers.md),
[value-reads-to-open](value-reads-to-open.md), [scope-reads-to-open](scope-reads-to-open.md) — sweep
many more sites, and one may hold a re-anchored reference that genuinely must ride up the call stack
and cannot fold into a closure. `open` forbids escape by construction (its `for<'b>` brand is
un-nameable in the result), so such a site has no `open` route; a borrow-bounded accessor —
re-anchoring **capped at** the witness borrow `'w` rather than a free `'b` widenable past the pin —
would be the sound fallback. Whether any site actually needs it is unknown until the migrations run.

**Acceptance criteria.**

- The call-site migrations are surveyed for any re-anchored reference that must escape the dispatcher
  call stack and cannot nest under the keystone's `open`.
- If any such site exists: `Sealed<T, W>` gains a borrow-bounded
  `attach<'w>(&'w self, &'w W) -> Live<'w, T>` over the externally-witnessed sealed form —
  re-anchoring capped at the witness borrow, the shape of the shipped
  [`vend_carrier`](../../src/witnessed.rs) / [`reattach_ref_with`](../../src/witnessed.rs) — with its
  own Miri tree-borrows proof (round-trip, and refuses-when-the-anchor-is-widened), and the
  un-nestable site(s) are named with why they cannot fold.
- If no such site exists: `attach` is not added, the survey's conclusion that every site nests under
  `open` is recorded, and [remove `attach`](remove-attach.md) closes as a no-op.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Necessity is contingent on the migrations — open.* The keystone proved the run-loop tail nests
  without escape; whether any other site genuinely cannot is settled by the migrations.
  Recommended: prefer `open` + copy-out everywhere and reach for `attach` only on a site that
  demonstrably cannot nest, so the verb is added only if earned.
- *Borrow-bounded, not free content — decided.* If added, `attach` re-anchors capped at the witness
  borrow `'w` — the witness pin outlives it, a fact the compiler checks — never a free `'b`
  widenable past the pin; that escaping shape is the keystone's rank-2 `open`, not `attach`.

## Dependencies

**Requires:**

- [Consuming externally-witnessed `open` and the run-loop step restructure](runloop-cps-open.md) —
  supplies the externally-witnessed sealed form and the `open` whose insufficiency `attach` backstops.

**Unblocks:**

- [Remove `attach`](remove-attach.md) — if `attach` is added, its removal is the cleanup; if not,
  that item closes as a no-op.
