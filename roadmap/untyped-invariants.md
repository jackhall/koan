# Promote untyped invariants into the type system

**Problem.** Runtime invariants across the codebase are enforced by caller
discipline plus runtime panics rather than by the type system. A dozen-plus
`panic!("expected \`xs\` bound to a List", ...)` sites in
[`interpret.rs`](../src/runtime/machine/execute/interpret.rs) assert a variant
tag a binding "must" have; `NodeStore::read(id)` asserts `NodeOutput::Value(_)`
rather than returning a `Result`; `Wrapped.inner` is invariantly not a
`Wrapped` only by construction-site discipline; `CallArena`'s `'static`
transmute is sound only because nothing moves its `Rc`'d payload. The survey
below catalogs the most load-bearing of these. Each entry is a candidate for
promotion into the type system whose footprint is local to one module.

**Impact.**

- *Failure modes shift from runtime panic to compile-time error.* Promoted
  invariants are caught by `cargo check`, not by the test suite's coverage of
  unusual paths.
- *Constructor and accessor APIs become self-documenting.* A
  `NodeId<Pending>` returned from `take_for_run` reads as the contract
  "completes once before consumption" without a doc comment.
- *Phase-ordered passes become unambiguous at the type level.* Phase-tagged
  carriers (`TypeNameRef<Resolved>`) make "did Stage-2 run?" a type check, not
  a memo-cell inspection.
- *Substrate for a future static-typing pass.* That pass needs to know which
  carriers are resolved at entry; the `TypeNameRef` phase tag is on the
  critical path.

**Directions.**

- *Collect as independent elements rather than one cohesive design — decided.*
  No element below requires another. The priority list sequences only by
  leverage and blast radius.
- *Per-element type-system mechanism — open.* Each element names the recommended
  shape (newtype, typestate, phantom tag, index newtype) inline alongside its
  trade-offs.

## Elements

### Phase tag on `TypeNameRef` (resolved vs unresolved)

**Where.** [`kobject.rs:87-92,215`](../src/runtime/machine/model/values/kobject.rs),
[`fn_def/tests/return_type.rs`](../src/runtime/builtins/fn_def/tests/return_type.rs);
also the `KTypeValue` vs `TypeNameRef` variant split in `TypeExprRef` slots.

`TypeNameRef`'s `OnceCell<&'a KType>` memoizes resolution and resets on
`deep_clone` across scope boundaries; Stage-2 elaboration is supposed to fill
the cell before any consumer sees the carrier. Promote with a phase parameter
— `TypeNameRef<Unresolved>` vs `TypeNameRef<Resolved>` — so Stage-2 elaboration
returns the resolved variant and downstream consumers cannot pattern-match an
unresolved carrier at all.

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

### `NodeStore` temporal-ordering typestate

**Where.** [`node_store.rs:97,175,210`](../src/runtime/machine/execute/scheduler/node_store.rs),
[`finish.rs`](../src/runtime/machine/execute/scheduler/finish.rs).

Four sites assume a temporal ordering the type can't express: `take_for_run`
("scheduler must not revisit a completed node"); `read(id)` asserts
`NodeOutput::Value(_)` rather than returning `Result`; `result_slot` is only
safe after the notify-walk wakes the consumer; the `finish` Lift wake assumes
`results[from]` is `Some`. Promote with a typestate on node handles —
`NodeId<Pending>` vs `NodeId<Completed>` — so `read` / `result_slot` accept
only completed handles and `take_for_run` consumes a `Pending`, returning
either a `Completed` or nothing.

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

### Variant-tag accessors in `interpret` / `type_ops`

**Where.** [`interpret.rs:160-574`](../src/runtime/machine/execute/interpret.rs),
[`type_ops.rs`](../src/runtime/builtins/type_ops.rs).

A dozen-plus `panic!("expected \`xs\` bound to a List", ...)` sites assert a
binding looked up by name is a specific `KObject` variant; slot values in
type-ops are asserted to be `KTypeValue` / `KSignature` / `KModule` ("Wrap
must be a TypeConstructor"). No type-level dispatch on the variant. Promote
with typed accessors that return `Option<&List>` / `Option<&KSignature>` and
propagate `KError` on mismatch, or by lifting the dispatch into the signature
so the slot type encodes which variant is required.

### Parser arity types (remaining unwraps)

**Where.** [`operators.rs:33,38,39,44`](../src/parse/operators.rs),
[`dict_literal.rs:35`](../src/parse/dict_literal.rs),
[`type_expr_frame.rs:153,186`](../src/parse/type_expr_frame.rs).

The `expression_tree.rs` frame-variant unwraps are gone (post-types-refactor:
`ParseStack` plus `pop_if_list/dict/type` and `pop_top` helpers; two surviving
`.expect`s at `]` / `}` close are forced by the flush-token ordering between
variant check and pop). What remains: four `ops.pop().unwrap()` for
prefix/infix arity in `operators.rs`, one pair-count parity assumption in
`dict_literal.rs`, two "exactly one" `into_iter().next().unwrap()` sites in
`type_expr_frame.rs`. Promote with the same shape-checked-pop pattern `ParseStack`
introduced — a `NonEmptyOpStack` or a `consume_arity::<N>()` method that
returns a fixed-size tuple rather than N pops.

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

### Multi-part expression shape at builtin sub-eval

**Where.** [`cons.rs:80,84`](../src/runtime/builtins/cons.rs).

`is_multi` guarantees every part is `Expression(_)` and `len >= 2`; the
`unreachable!` and `expect` rely on the caller having filtered first. AST-
level, not `BodyResult`-level. Promote with a multi-part-expression type the
caller constructs via a smart constructor that enforces both invariants, then
hand the typed shape to the sub-eval site.

## Priority

1. **Phase tag on `TypeNameRef`** — eliminates an entire class of "did Stage-2
   actually run?" bugs; any future consumer that wants to assume a resolved
   type at a slot boundary depends on it.
2. **`NodeStore` temporal-ordering typestate** — eliminates the four most
   load-bearing `panic!`s in the scheduler; the typestate pattern is
   well-trodden and the scheduler's test surface is large enough to catch
   regressions.

The remaining elements are sequenced by opportunity, not dependency — pick
whichever sits adjacent to other work in the same file or module.

## Dependencies

**Requires:** none — each element is local to its named module and can land
independently.

**Unblocks:** none directly. The `TypeNameRef` phase tag is on the critical
path for any future analysis pass
([Static type checking and JIT compilation](static-typing-and-jit.md)) that
wants to assume elaboration has run before its analyses begin.
