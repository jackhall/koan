# Encode `CallArena` heap-pinning in the type system

**Problem.**
[`CallArena`](../src/runtime/machine/core/arena.rs) heap-pins its scope via
`Rc`, and the `'static` transmute at the arena boundary is sound only because
nothing moves the `Rc`'d payload — a no-move property nowhere encoded in the
type. The invariant is load-bearing across the runtime:
[`KFunction<'a>`](../src/runtime/machine/core/kfunction.rs) holds
`NonNull<Scope<'a>>` and relies on caller discipline at `with_pre_run`;
`escape_ptr` chains link outer arenas through raw pointers; module child-scope
pointer validity in [`module.rs`](../src/runtime/machine/model/values/module.rs)
rides on the same heap-pinning. Every site lives in caller discipline and a
`// SAFETY:` comment rather than in the type.

**Impact.**

- *The no-move contract is statically enforced.* `cargo check` rejects code
  that could move the pinned payload, replacing audit-only soundness with
  type-system soundness.
- *Constructor and accessor APIs become self-documenting.* A
  `Pinned<Scope>` returned from `CallArena::new` reads as the contract
  "the scope never moves while this handle is live" without a doc comment.

**Directions.**

- *Type-system mechanism — open.* Candidates: a `Pinned<Scope>` newtype
  around the `Rc` with a single `as_ref` API and the raw pointer never
  escaping; `std::pin::Pin<Rc<Scope>>` with a structural-pinning wrapper;
  a phantom-tag invariance trick. Recommended: bespoke `Pinned<Scope>`
  newtype — the API surface is narrow enough that `Pin` ergonomics are
  more friction than help.

## Dependencies

**Requires:** none.

**Unblocks:** none.
