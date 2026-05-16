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

### `Bindings` / `Scope` state-machine encapsulation

**Where.** [`scope.rs:36-38,228-280`](../src/runtime/machine/core/scope.rs),
[`bindings.rs`](../src/runtime/machine/core/bindings.rs),
[`struct_def.rs:63,87,107,110`](../src/runtime/builtins/struct_def.rs).

Several coherence invariants on `Bindings`/`Scope` are enforced by caller
discipline rather than the type: `data` and `placeholders` never both hold the
same name (relies on `bind_value` doing remove+insert atomically); every
`data[name]` wrapping a `KFunction` mirrors into `functions[sig_key]` (relies
on `try_register_function` being the sole writer); `pending_types` register/
remove lifecycle around Stage-3.2 SCC; `cycle_close_install_identity` and
`register_nominal` panic on borrow conflicts and pre-existing `types` entries,
with the "post-Combine, non-re-entrant" phase ordering external to the type.
Promote with a `Bindings` API that hides the multi-map structure behind
single-writer methods, and a phase witness (e.g. a `PostCombine<'a>` token
mintable only by the scheduler) threaded into the cycle-close path so its
panicking branches become statically unreachable.

### Index newtypes for allocator-managed arrays

**Where.** [`expression_tree.rs:83`](../src/parse/expression_tree.rs),
[`kfunction.rs:420-421`](../src/runtime/machine/core/kfunction.rs),
[`node_store.rs:95,103,130,137,145-162`](../src/runtime/machine/execute/scheduler/node_store.rs).

`map.get(&idx)` on the parser frame map, argument-position lookup after a
"missing-arg check above guarantees presence" comment, direct `nodes[idx]` /
`results[idx]` indexing relying on allocator/free-list discipline. Promote
with index newtypes the allocator hands out and indexed-collection wrappers
(e.g. `IndexedVec<NodeId, Node>` with typed `Index` / `IndexMut`) so the
"presence" claim is encoded by the index's existence.

## Priority

1. **`Bindings` / `Scope` state-machine encapsulation** — bounded to the
   binding/scope mutation surface; a `PostCombine<'a>` phase witness has a
   clear mintable-only-by-scheduler shape that turns several panic branches
   statically unreachable.
2. **Index newtypes for allocator-managed arrays** — moderate blast radius
   (touches `NodeId`-typed call sites across the scheduler); the
   `IndexedVec<NodeId, Node>` shape encodes presence at index handout time
   so the "missing-arg check above guarantees presence" comments become
   the index's existence.

Arena lifetime and heap-pinning discipline is sequenced last (not listed
above) — highest blast radius, deepest into `unsafe`, and load-bearing
across the runtime; best taken on after the cheaper elements clear the
surrounding noise.

## Dependencies

**Requires:** none — each element is local to its named module and can land
independently.

**Unblocks:** none.
