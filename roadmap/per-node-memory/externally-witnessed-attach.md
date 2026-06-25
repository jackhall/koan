# Externally-witnessed sealed form and `attach`

Add the witness-supplied-at-access shape to `Sealed`, with its own Miri proof, and reimplement
the shipped witness-borrow reattaches on top of it.

**Problem.** A [`Sealed<T, W>`](sealed-open.md) bundles its witness `W`. A carrier whose backing
the holder already pins (the per-call child scope; a continuation read against the frame `Rc`)
has no way to be sealed *without* a redundant bundled witness — bundling a reference-counted
clone would add an owner the holder's own uniqueness checks must subtract. The shipped
witness-borrow reattaches that serve these cases —
[`vend_carrier`](../../src/witnessed.rs) and `reattach_with` / `reattach_ref_with` /
[`reattach_slice_with`](../../src/witnessed.rs) — are loose functions over `Erased`, not a method
on a sealed node-storage form.

**Acceptance criteria.**

- `Sealed<T, W>` has a witness-less form, built without moving a witness into the bundle, read
  through `attach<'w>(&'w self, &'w W) -> Live<'w, T>` — re-anchoring **capped at** the witness
  borrow `'w`, exactly as the shipped [`vend_carrier`](../../src/witnessed.rs) (`-> T::At<'w>`) and
  [`Witnessed::read`](../../src/witnessed.rs) (`-> T::At<'_>`) do; a reference carrier returns the
  `&'w T::At<'b>` form of [`reattach_ref_with`](../../src/witnessed.rs) (borrow `'w`, content `'b`
  free). The content does **not** ride a free `'b: 'w` that outlives the witness — that escaping
  shape is `open`'s rank-2 `for<'b>` brand, not `attach`'s.
- `attach` carries a self-contained Miri tree-borrows proof (round-trip, and
  refuses-when-the-anchor-is-widened) distinct from `open`'s rank-2 brand.
- The shipped `vend_carrier` / `reattach_*_with` functions are reimplemented as thin delegates
  to `Sealed::attach`, so their call sites compile unchanged while the method becomes the single
  witness-borrow primitive.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Primitive is the method form of a shipped wrapper — decided.* `attach` is the `Sealed`
  method form of the shipped `vend_carrier` / `reattach_with` witness-borrow bound; the soundness
  shape is proven, so this item is the sealed-form type plus the delegating reimplementation, not
  a new `unsafe` argument.
- *`attach` is transitional — decided.* The substrate's destination is the single `open` verb;
  `attach` exists so a re-anchored reference can ride up the dispatcher call stack without a copy
  during migration, and is retired by [remove `attach`](remove-attach.md). The call-site
  retirements (the [wrapper migration](migrate-reattach-helpers.md)) prefer
  `open` + copy-out and reach for `attach` only where a reference genuinely escapes, to minimize
  the double-touch before removal.

## Dependencies

**Requires:**

- [Sealed node-storage carrier and `open`](sealed-open.md) — the `Sealed` type and `open` this
  extends.

**Unblocks:**

- [FrameStorage self-reference removal](framestorage-self-reference.md) — the per-call child
  scope is its first production consumer.
- [Migrate the loose witness-borrow wrappers onto `Sealed`](migrate-reattach-helpers.md) — retires
  the wrappers this reimplements.
