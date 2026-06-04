# Type-enforced scope re-attach

Brand `ScopePtr` with the carrier's lifetime so the scope re-attach is safe to call,
and the carrier accessors drop their `unsafe` blocks.

**Problem.** [`ScopePtr::reattach`](../../src/machine/core/scope_ptr.rs) is an
`unsafe fn` that returns a free `'a`. Every carrier accessor — `Module::child_scope`,
`Signature::decl_scope`, `KFunction::captured_scope`
([kfunction.rs](../../src/machine/core/kfunction.rs)), and `CallArena::scope`
([arena.rs](../../src/machine/core/arena.rs)) — wraps it in an `unsafe` block and
re-states the "arena-pinning keeps the pointee alive for `'a`" argument in a SAFETY
comment. The `'a`-invariance pins (`KFunction` / `Signature`
`PhantomData<&'a Scope<'a>>`) are hand-maintained beside the pointer, guarded by a
"do not simplify" warning rather than a structural guarantee.

**Impact.**

- `Module`, `Signature`, and `KFunction` read their captured scope through a plain
  safe method: a lifetime brand makes the re-attach sound to call, so the soundness is
  enforced by the type rather than re-argued at each site.
- The `'a`-invariance of `KFunction` and `Signature` is carried structurally by the
  branded pointer, retiring the hand-maintained `PhantomData` pins.
- The irreducible `unsafe` surface concentrates at `CallArena`'s non-generic
  `'static → 'a` boundary — the one place lifetime fabrication originates — so a reader
  auditing memory safety has a single trusted core to check.

**Directions.**

- *Brand mechanism — open.* A `ScopePtr<'a>` carrying `PhantomData<&'a Scope<'a>>`,
  with a safe `erase(&'a Scope<'a>) -> Self` (records the input lifetime, so it cannot
  fabricate a longer one) and a safe `reattach(&self) -> &'a Scope<'a>` (the transmute
  is internal, justified by the brand). The obstacle is `CallArena`: it is non-generic
  by design (it backs `Rc<CallArena>`, which carries no lifetime), so it has no `'a` to
  brand and must keep an `unsafe` `'static → 'a` fabrication for `scope` /
  `scope_for_bind` / `anchored_parts`. Options: (a) `CallArena` stores `ScopePtr<'static>`
  and its accessors are the single documented unsafe boundary while the carriers go
  safe; (b) a separate unsafe free-`'a` method scoped to `CallArena`; (c) a different
  brand carrier. Recommended: (a) — concentrate the irreducible fabrication at
  `CallArena`, where erasure originates, and let everything downstream of it be safe.
- *Variance preservation — decided.* The brand must keep `KFunction` and `Signature`
  invariant in `'a` (`Scope<'a>` is invariant; covariance silently reintroduces a
  use-after-free). The `PhantomData<&'a Scope<'a>>` inside `ScopePtr<'a>` carries that
  invariance for the carrier, but the replacement must be variance-checked, not assumed.
- *Validation — decided.* Re-run the full Miri slate before and after under tree
  borrows via the `miri` skill; the slate's `ScopePtr` group already pins the re-attach,
  so a regression in the brand surfaces there.

## Dependencies

**Requires:** none — builds on the shipped `ScopePtr` / `anchored_parts` arena
consolidation, but adds no new prerequisite.

**Unblocks:** none tracked yet.
