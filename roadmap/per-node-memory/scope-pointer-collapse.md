# Collapse the scope-pointer erasure into the substrate

Re-anchor a region-resident value's captured / defining / parent scope through the holder's own carrier
`open`, deleting the scope-specialized `BoundedScopePtr` / `ErasedScopePtr` handles and the bare
`reattach_ref` so the scope path's only `unsafe` is the substrate's single `retype`.

**Problem.** A region-resident value's scope reference — `KFunction::captured`, `Module` /
`Signature`'s scope pointers, and a `Scope`'s `outer` lexical parent and `root` — is stored through a
scope-specialized erasure parallel to the substrate
([`scope_ptr.rs`](../../src/machine/core/scope_ptr.rs)). `BoundedScopePtr<'a>` holds a
`NonNull<Scope<'static>>` and re-hands it through `get`, which routes a `NonNull::as_ref` deref **plus**
the bare unsafe [`reattach_ref`](../../src/witnessed.rs) — the scope path's only `unsafe` beyond the
substrate's one `retype`. `ErasedScopePtr` and the frame's `SealedExtern<ScopeRefFamily>` re-hand the
lifetime-free scope through the witness-bounded [`reattach_ref_with`](../../src/witnessed.rs). The
[`module.rs`](../../src/machine/model/values/module.rs) /
[`memory-model.md`](../../design/memory-model.md) framing is inverted: it calls `BoundedScopePtr`
"safe" and `ErasedScopePtr` the "irreducible unsafe," when `ErasedScopePtr` is the safe-signature one
(only the shared `retype`) and `BoundedScopePtr::get` carries the extra `NonNull` deref. The scope a
holder reaches is a foreign borrow the holder's witness already pins (a co-located capture, or an
ancestor pinned by the `outer` chain), so it can re-anchor as part of the holder's own carrier `open` —
once the holder opens at a brand.

**Acceptance criteria.**

- A region-resident value's captured / defining / parent scope re-anchors as part of the holder's own
  carrier `open` at the brand — one `Reattachable` retype over the whole value — on the `&'static Scope`
  representation (held outright, no `NonNull`).
- `BoundedScopePtr` and `ErasedScopePtr` are deleted, along with the brand-shortening helpers
  (`erase_shortened` / `shortened`) and the bare `reattach_ref`.
- The scope / module / function path carries no `unsafe` of its own: the only `unsafe` it reaches is
  the substrate's single `retype`, and the inverted "irreducible unsafe" comment in `module.rs` is
  corrected.
- TCO frame reuse is unaffected — `try_reset_for_tail` keeps its three Miri tests.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Re-anchor through the holder's `open`, not a per-pointer handle — decided.* A scope reference is a
  foreign borrow the holder's witness pins, so opening the holder's carrier at the brand re-anchors it
  with the rest of the value through one substrate `retype`; the scope-specialized handle is removed,
  not relabelled.
- *`&'static Scope`, not `NonNull` — decided.* `ErasedScopePtr` already proves a held `&'static Scope`
  survives `typed_arena` growth under tree borrows and re-anchors with no `as_ref`, so the `NonNull`
  representation (and its deref `unsafe`) is unnecessary. The brand-shortening helpers reconcile stored
  `'a` relationships that erase-to-`'static` makes moot, so they go with it.

## Dependencies

**Requires:**

- [Fold the scope channel into the step `open`](scope-reads-to-open.md) — a holder's `outer` / `root`
  re-anchors through the holder's own `open`, which the scope channel's fold establishes.

**Unblocks:**

- [`Sealed`: a single access verb](single-open-verb.md) — removing `ErasedScopePtr` clears one of
  `reattach_ref_with`'s callers.
