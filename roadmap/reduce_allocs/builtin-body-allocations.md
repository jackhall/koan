# Builtin body allocations

The builtin bodies allocate on the heap where the step scratch or an interned symbol would do.

**Problem.** `src/builtins/` holds roughly 180 production allocation sites. Most are `format!`
inside a `KError` on a failure arm and cost a correct program nothing, but four kinds sit on paths
a program actually walks, and none of them is priced by a decision — each is a buffer or a
rendering that could have been free.

Ten transient `Vec`s are built, read, and dropped inside one step while `ctx.scratch`
([`BodyCtx::scratch`](../../src/machine/core/kfunction/action.rs), the drain's per-pop bump) sits
unused beside them: `pins` in [type_ops/with.rs](../../src/builtins/type_ops/with.rs), `names` in
[record_projection.rs](../../src/builtins/record_projection.rs), `members` in
[type_union.rs](../../src/builtins/type_union.rs), `tags` in
[ascribe.rs](../../src/builtins/ascribe.rs), the `writes` and the wake-time `resolved` in
[fn_def/finalize.rs](../../src/builtins/fn_def/finalize.rs), three `Vec<WriteOp>` across
[fn_def/finalize.rs](../../src/builtins/fn_def/finalize.rs) and
[op_def.rs](../../src/builtins/op_def.rs), and the await staging pair in
[fn_def/signature.rs](../../src/builtins/fn_def/signature.rs). The `WriteOp` vectors are a
round trip through the heap to reach a bump the destination already is: `Action::with_effects`
takes an `IntoIterator` and extends a `BumpVec::new_in(scratch)`.
[branch_walk.rs](../../src/builtins/branch_walk.rs) already stages its arm and head buffers on
`scratch` and is the only file in the directory that does.

Two more in [fn_def/finalize.rs](../../src/builtins/fn_def/finalize.rs) look transient and are
not: `splice_layout` and `prebuilt_elements` are moved into the `Box<dyn FnOnce>` await
continuation, which runs at a later drain pop than the one whose scratch would host them, so a
`BumpVec` on the step arena would dangle. Their element types are `Copy` — `(usize, usize)` and
`SignatureElement` — so their home is the frame region's bump, which the continuation's own `'a`
already outlives, rather than the heap.

Seven sites clone a whole `TypeNode` to read one field. `TypeRegistry::node`
([src/machine/model/types/registry.rs](../../src/machine/model/types/registry.rs)) is
`with_node(handle, TypeNode::clone)`, and a `Signature` node owns a schema, so
`matches!(ctx.types().node(*kt), TypeNode::Signature { .. })` in
[let_binding.rs](../../src/builtins/let_binding.rs) and
[ascribe.rs](../../src/builtins/ascribe.rs) allocates a member map and drops it to compute a
`bool`. Two more in `ascribe.rs` clone per iteration of a loop over a signature's slots.

Six sites render an interned label back to a `String` where the symbol would serve.
`WriteOp::Overload` ([src/machine/core/bindings/ops.rs](../../src/machine/core/bindings/ops.rs))
carries a `String` `name`, alone among the variants — `Value`, `Type`, and `Group` all carry
symbols — so every `FN` and `OP` declaration pays a `LabelInterner::resolve` at
[fn_def/finalize.rs](../../src/builtins/fn_def/finalize.rs) and
[op_def.rs](../../src/builtins/op_def.rs) to produce it. The field keys nothing: the bucket is
keyed by `seal.key`, and `name` reaches only the `DuplicateOverload` and `Rebind` diagnostics and
a `Debug`-formatted `debug_assert`. Its three producers do not even agree on what it names —
`finalize.rs` renders the signature's *first* keyword, `op_def.rs` the operator glyph, and
`Scope::adopt_registration` the whole untyped bucket key. [module_def.rs](../../src/builtins/module_def.rs) and
[group_def.rs](../../src/builtins/group_def.rs) render a binder's spelling unconditionally on the
success path for pre-scan helpers that quote it only in diagnostics.
[attr.rs](../../src/builtins/attr.rs) always takes `Cow::Owned` for a classified field name, and
renders a type name to text purely to re-classify and re-hash it on an arm its own comment says
"names no member either". Underneath all six, `LabelInterner::resolve`
([src/machine/model/labels.rs](../../src/machine/model/labels.rs)) hands back a fresh `String`
copy of a `Box<str>` the table already owns; the no-allocation `display` view exists beside it and
these callers do not reach it.

And three sites copy where no copy is wanted. `PRINT`
([src/builtins/print.rs](../../src/builtins/print.rs)) renders three copies of its output per
call — `summarize` to a `String`, a `format!` whose whole job is appending one byte, then the
region copy that is kept — while `RunWriter::write_out` takes `&[u8]` and would take the newline
as a second call. `A | B` ([type_union.rs](../../src/builtins/type_union.rs)) heap-allocates a
two-element `Vec` per evaluation because `TypeRegistry::union_of` takes `Vec<KType>` by value and
only iterates it; `union_of` then builds a second `flat` vector that is discarded whenever the
resulting node is already interned, which is the steady state inside a loop.

`record_projection`, `attr`, and `let_binding` are in the `wide` composite's body, so their share
is in the `wide_step` term ([observe/alloc.txt](../../observe/alloc.txt)); the
`WriteOp::Overload` rendering is in `declare_name`. The rest are unpriced by the recorded shapes:
`PRINT` runs once per program at top level, `ascribe` once, and nothing in any shape reaches
`A | B`, `WITH`, `OP`, or `GROUP`.

**Acceptance criteria.**

- No builtin body stages a transient buffer on the global heap: every `Vec` built, read, and
  dropped within one step is a `BumpVec` over `ctx.scratch`, and no builtin builds a
  `Vec<WriteOp>` to hand to `Action::with_effects`. The one exception is `awaited`, whose fill has
  no reservable bound — see Directions.
- No builtin body stages a buffer on the global heap that outlives its step either: a buffer an
  await continuation carries is a `BumpVec` over the frame region it is already confined to.
- No builtin calls `TypeRegistry::node` to read a field or test a discriminant it could read
  under `with_node`.
- `WriteOp::Overload` carries no text, and registering an overload renders no label text on the
  success path; the diagnostics that name a colliding or shadowing overload render its full
  untyped bucket key on their own arm.
- No builtin renders a label or type name to a `String` on a path that succeeds; a spelling a
  diagnostic quotes is rendered on the failure arm or through `LabelInterner::display`.
- `PRINT` writes its rendered value and its newline without an intermediate `String`.
- `TypeRegistry::union_of` takes its members as a slice, and evaluating an already-interned
  union allocates nothing.
- `wide_step` and `declare_name` both fall against the recorded sweep, and
  `tests/allocation_baseline.rs` is rebaselined to the new figures with its headroom intact.

**Directions.**

- *Scratch relocation — decided.* `BumpVec::with_capacity_in(n, ctx.scratch)` at each of the ten
  sites, following [branch_walk.rs](../../src/builtins/branch_walk.rs). Where a helper returns
  `(carrier, writes)` rather than building in the body, thread the allocator in the way the
  `Action` write-attachment door already does. Each buffer's consumer takes a slice or an
  iterator today, so no consumer signature changes.
- *Reservation exactness — decided.* Each relocated fill takes its enclosing loop's length as its
  capacity. Most are exact; `pins` and `names` reserve an upper bound, which is what the
  no-regrow property needs.
- *`awaited`'s unbounded fill — deferred.* `awaited` in
  [fn_def/signature.rs](../../src/builtins/fn_def/signature.rs) keeps its heap allocation. Its
  length is data-dependent with no upper bound available at the fill site — it grows by
  `extend(producers)` over a `TypeResolution::Park(Vec<ProducerId>)`
  ([src/machine/model/types/resolver.rs](../../src/machine/model/types/resolver.rs)) of arbitrary
  length — so a `parts.len()` reservation could be outgrown and abandon its buffer as dead region
  bytes. It is recorded under [Unplanned work](README.md#unplanned-work) beside the other producer
  lists, which is where a reservation discipline for the whole tier belongs. Its peer
  `sub_dispatches` pushes once per signature part and relocates as an exact fill.
- *`node` → `with_node` — decided.* Local, one call each, no signature changes. The two
  `matches!` discriminant tests are the clearest and go first.
- *The `WITH` schema clone — deferred.* [type_ops/with.rs](../../src/builtins/type_ops/with.rs)
  clones a `TypeNode::Signature` per evaluation, the largest single per-eval clone in the
  directory, and the clone is load-bearing: the schema is read across a `fold_pins` that interns,
  and `TypeRegistry::intern` rejects being reached from inside a `with_node` borrow. Closing it
  means restructuring the registry's read/intern split, which is its own item; this one leaves
  the site alone and records why.
- *`WriteOp::Overload { name }` — decided.* Delete the field rather than retype it. It keys
  nothing, and a `KeywordSymbol` could not carry what `adopt_registration` puts there — a
  multi-keyword bucket key. Both diagnostic sites render `render_untyped_key(&seal.key, …)` on
  their own arm from the seal they already hold, so the collision message names the whole key
  (`(DOUBLE _)`) rather than a lead keyword. `Scope::register_function_direct` drops its `name`
  parameter with it.
- *The `resolve` door — decided.* Unchanged, and no new door. Deleting `WriteOp::Overload`'s
  `name` removes both of `resolve`'s builtin callers; `attr.rs` recovers its symbol through the
  existing [`KType::name_symbol`](../../src/machine/model/types/ktype.rs) instead of rendering
  and re-classifying; and `module_def.rs` / `group_def.rs` thread the binder's symbol to their
  helpers and render through `display_label` on the error arms. Nothing is left that needs the
  text without the ownership.
- *`union_of` — decided.* Take `&[KType]`, so `A | B` passes a stack array, and build `flat` in a
  `SmallVec`, materializing the owned `Vec` only on an intern miss. Same treatment for
  `intern_union_flat`.
- *`summarize` — deferred.* `PRINT`'s remaining copy is `Held::summarize` returning a `String`
  ([src/machine/model/values/carried.rs](../../src/machine/model/values/carried.rs)); a `Display`
  shape would let the render go straight into the region text. The family spans `Held`, `Carried`,
  `KObject`, `KType::name` and the `Shape` trait, with 73 production callers — its own item,
  [Surface rendering as a `Display` view](surface-rendering-display.md). Here `PRINT` drops the
  `format!` and nothing else.
- *`WriteOp::Group { probes }` — deferred.* `powerset_probes`
  ([src/machine/core/bindings/ops.rs](../../src/machine/core/bindings/ops.rs)) collects into a
  heap `Vec` per operator-group registration. A `WriteOp` is applied at the run loop rather than
  in the deciding body, so bumping the probe list needs its lifetime checked against apply
  first — out of scope here.
- *Failure arms — decided.* Untouched. The `format!`-in-`KError` sites, the partition-guard
  suggestion builders in [let_binding.rs](../../src/builtins/let_binding.rs), the inexhaustive
  diagnostic's `join` in [branch_walk.rs](../../src/builtins/branch_walk.rs), and the
  `MissingArg("…".to_string())` pattern all cost a correct program nothing.
- *Registration `vec![…]` — decided.* Untouched. Every builtin's `register()` runs once per
  process at seeding, and [audit/README.md](../../audit/README.md) already treats the
  registered-overload count as the fixed term every differencing pair cancels.
- *`AwaitContinue` boxes — decided.* Untouched. The continuation outlives the decide, so
  `Box<dyn FnOnce…>` is the design; bumping it means retyping the await protocol, not a local
  cleanup.
- *Sequencing — decided.* One builtin (or one family of related builtins) per phase, each
  ending on a green slate and its own commit, with `python3 tools/alloc_audit.py` between each.
  The `WriteOp::Overload` deletion goes first and alone, since the `FN` and `OP` declaration
  paths both feed it. Phases that touch a path no recorded shape walks are held to a flat sweep
  plus a stated site-level claim rather than a moved term.

## Dependencies

**Requires:** none — every change is local to a builtin body or to a door it already calls.

**Unblocks:** none tracked yet.
