# Miri audit slate

<!-- slate-fingerprint

-->

The canonical list of tests Miri's tree-borrows mode signs off on for koan's
memory safety. Each test is a minimal-shape mirror of an unsafe site in the
runtime; the slate passes when Miri reports zero process-exit leaks and zero UB
across the whole list.

Command of record and triage workflow live in
[.claude/skills/miri/SKILL.md](../.claude/skills/miri/SKILL.md). Memory-model
invariants the slate verifies live in
[design/memory-model.md](../design/memory-model.md).

## Stale-group whitelist

Slate groups whose anchor file no longer carries `unsafe` because the test pins
a safe-code invariant (e.g. a `RefCell` discipline that tree borrows can still
violate). `slate-audit` skips the stale-group check for these paths only; new-
unsafe and fingerprint-drift checks still fire.

**Whitelisting is not automatic when an `unsafe` is removed or relocated.** A slate
test earns its place — and a whitelist entry — only if it can catch a memory error
*no other slate test catches*. When an `unsafe` site is deleted (or its backing op
moves to another file, e.g. a layout-invariance `unsafe impl` folded into the
`reattachable!` macro whose home is `witnessed.rs`), ask of each test under the now
anchor-less group: does it still pin a distinct UB shape? If yes — keep it and
whitelist the anchor here, citing the shape and where the real `unsafe` now lives. If
no — the test is redundant; **delete it** rather than whitelist. Do not whitelist a
group just to silence the stale-anchor check.

<!-- slate-audit-whitelist:start -->
- `src/machine/model/types/typed_field_list.rs` — the declaration-window sibling-cell group pins a
  pin-less read (`read_resting`) whose coverage is a *parked* step's region, not the reading step's —
  an ordering claim no other slate test makes. The real `unsafe` is the `Sealed::open_with` retype in
  `witnessed.rs`; typed_field_list.rs carries none of its own.
- `src/machine/core/arena.rs` — arena.rs split into `arena/{frame,step_allocator}` child
  modules. Its groups (CallFrame lifetime erasure, the record substrate door, MATCH-Tagged /
  TRY-WITH TCO, per-call frame
  re-anchor, NodeStore reinstall) pin safe-code frame / carrier / region drop-order and reattach
  disciplines whose backing `unsafe` is the `Region::alloc_resident` retype in `witnessed.rs`. Every
  `KFunction` / `Scope` / `Module` store reaching that retype rides a born door
  (`RegionHandle::alloc_resident_born_with`), whose `for<'b>` brand discharges residence at compile
  time — so koan-side `src/` carries no `unsafe` of its own at all.
- `src/machine/core/scope.rs` — `Scope::add` re-entry pins the queue-and-drain
  discipline that keeps `Scope`'s `RefCell<…>` invariant intact when a binding
  is added while a `data` borrow is live.
- `src/machine/core/kfunction.rs` — `KFunction::captured_scope` is a bare field read of the
  stored `&'a Scope<'a>` (re-anchored with the holder by the `Region::alloc_resident` retype), so
  kfunction.rs carries no `unsafe` of its own. The group pins the captured-scope-survives-
  closure-escape and delivered-carrier reach-fold shapes.
- `workgraph/src/scheduler/node_store.rs` — the slot-read group pins `read_result_with`'s
  `open_with` under the retained frame owner (a safe pinned open; the `unsafe` lives in
  `witnessed.rs`) via an end-to-end tail-chain return-contract-coarsening shape no
  minimal test reproduces. The file's only former `unsafe` was the test-family markers,
  now `reattachable!`-generated.
- `src/machine/core/ref_carriers.rs` — pointer-only group: every holder stores its captured /
  defining / parent scope as a plain `&'a Scope<'a>` re-anchored **with the holder as a whole** by
  the `Region::alloc_resident` retype in `witnessed.rs`, and the shape is pinned library-side by
  the workgraph slate's born-door group (invariant holder, foreign-region parent, post-store
  interior write). No koan test and no `unsafe` of its own.
- `src/machine/execute/nodes.rs` — pointer-only group: the `NodeScope::YokedChild`
  erase → open round trip (including a sibling store while the opened reference is live) is pinned
  library-side (`sealed_extern_open_externally_witnessed`,
  `an_erased_node_opens_and_survives_a_sibling_store_inside_the_open`); every scheduler-driving
  slate test exercises the production carrier end-to-end. No `unsafe` of its own.
- `src/machine/execute/outcome.rs` — pointer-only group: the fat-pointer (`Box<dyn>`)
  erase → open → invoke round trip is pinned library-side
  (`sealed_extern_open_invokes_a_fat_pointer_continuation`); the family's `unsafe impl` is
  `reattachable!`-generated, so outcome.rs carries none, and `run_step` runs the transmute
  end-to-end every step.
- `src/machine/model/values/module.rs` — pointer-only groups: the interior-mutation-under-a-live-
  `&'a Module` shape is pinned library-side (`the_returned_node_accepts_a_write_at_the_callers_lifetime`,
  `invariant_same_brand_mutation`), the koan `RefCell` round trips run under plain `cargo test`,
  and the MODULE-body Combine continuation rides the stored scope-pointer re-anchor the born-door
  group pins. No `unsafe` of its own.
- `src/machine/execute/dispatch/ctx.rs` — the `with_node_scope` read boundary is the
  sole production open of a `YokedChild` carrier; it passes the executing slot's
  cart `Rc` as the witness to `SealedExtern::open`, a **safe** call, so ctx.rs carries no
  `unsafe`. The group pins that boundary end-to-end (every scheduler-driving slate test); the
  `unsafe` it routes lives in `witnessed.rs`.
- `src/machine/execute/lift.rs` — `copy_carried` structurally copies at the brand a step open
  supplies (safe allocs throughout).
  The group pins the escaping-value **retention** discipline — a surviving closure / module borrow
  kept alive by the reach set the witnessed transfer mints into the destination — which tree
  borrows catches if it regresses.
- `src/machine/execute/run_loop.rs` — `run_step`'s dep-union `pin` is built entirely through safe
  envelope/`RegionSet` verbs (`Delivered::liveness_frameset`, `FrameSet::union`/`singleton`); the
  file carries no `unsafe` of its own. The group pins the retention redundancy claim — a dep's
  producer frame is held by its `DepTerminal`'s duplicated delivery envelope across the step open,
  not by `run_step`'s `pin` alone — the real `unsafe` it exercises is the shared `retype` in
  `witnessed.rs`, routed through the `Sealed`/`SealedExtern` opens `run_step` and the dep reads
  perform.
- `src/machine/core/scope/reach.rs` — the reach/carrier derivation cluster is safe code end to end:
  every store door composes through the library's envelope verbs (`merge_into_placing`,
  `transfer_into_placing`) and builds its product at a `FoldingBrand`, whose rank-2 signature makes
  an ambient-lifetime capture a compile error rather than a retype. The `unsafe` those verbs route
  is the shared retype in `witnessed.rs`. The group pins what no signature can prove — that the
  composition's **minted reach** names every region the product borrows, and that the release
  predicate over the rebuilt product is release-exact: adopt a loop-carried argument and free the
  retiring host one step early and the spliced carrier reads a dead region. Tree borrows is the only
  check on that arithmetic.
- `src/machine/model/values/kobject.rs` — the container doors are safe code (bumping a `&'a str`
  into a region borrowed for `'a` needs no retype), so the file carries no `unsafe`. The
  region-hosted-string group pins the **re-home rule** those doors implement: a string cell's
  `Owned` reach verdict names no region, so the door must re-bump the bytes at the destination or
  the cell points into a region nothing pins. No residence audit can catch that — the bump keeps no
  address table — which leaves tree borrows as the only check.
<!-- slate-audit-whitelist:end -->

## The slate

25 tests, grouped by the unsafe site (or the safe discipline routing it) each pins down. Names
below are the exact test identifiers; pass them after `--` in the Miri command. A further 44 tests
covering the witnessed substrate live in the `workgraph` crate's own slate
([workgraph/observe/miri_slate.md](../workgraph/observe/miri_slate.md)). The split rule: a shape
whose failure modes live entirely in the library's verbs (the region alloc engine, the envelope
transfer / duplication / adoption / re-stamp verbs, the mint's self rule and teardown, the
`alloc_with` dep fold, the born-door round trips, the `SealedExtern` opens over thin, boxed, and
fat-pointer carriers) is audited there, over library-only profiles; this slate holds only shapes
whose discipline lives in koan's own `src/` — its doors, seams, and scheduler-driving programs.

**`CallFrame` lifetime erasure** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — the
child-scope `Option<SealedExtern<ScopeRefFamily>>` opened at a `for<'b>` brand via `CallFrame::with_scope`
(`SealedExtern::open`, the frame's own storage `Rc` as the pin). The `Rc<CallFrame>` chain that keeps
per-call regions pinned across re-borrow is pinned library-side
(`the_born_with_door_accepts_the_childs_own_host_as_the_pin`, which stores a crossing operand under
the *destination's* own host and reads the embedded parent back after every direct handle on it
drops); `call_frame_chained_outer_frame_walkable` mirrors that shape over koan's `CallFrame` and
runs under plain `cargo test`. One test here pins
the **seed-side re-anchor** — a caller-lifetime value crossing into the opened scope's own region as
a delivery envelope, whose bind relocates it there, the shape the MATCH / TRY `it`-bind and the
user-fn param-bind take. A bare caller reference cannot cross the `for<'b>` signature at all, so the
envelope is the whole route. `CallFrame::adopting` (the scheduler-owned run
frame) carries the same `&Scope<'_>` erasure as `new`, over the run scope it adopts rather than a
freshly-minted child; it is built on the first run-lifetime submission, so every scheduler-driving slate
test below (`try_inside_tco_position_preserves_frame_chain`, `park_and_replay_minimal_program_for_miri`, …) exercises it
end-to-end — the run scope outlives the frame, so no separate minimal test. A second test pins the
**born door's own round trip** nested inside that open: a grandchild scope built and stored at the
frame brand (`Scope::alloc_child_under`, routing `RegionHandle::alloc_resident_born_with`) comes back
co-located, stays readable while its own brand appends to the same region, and still names its
parent — the erase-store / re-anchor sequence every `Scope` store now takes. It carries the
sibling-alloc claim in the same run: the opened child's re-borrow still names the frame's region
while a sibling pointer allocates into it, so `with_scope`'s `&Scope` and `brand().alloc(…)` are
pinned coexisting there rather than by a test of their own.

- `with_scope_relocates_seed_value_into_brand`
- `born_child_scope_survives_subsequent_alloc_in_its_own_region`

**Record substrate door — construction, O(1) ownership, fold-shared retype** ([src/machine/core/arena.rs](../src/machine/core/arena.rs))
— `FoldingBrand::alloc_substrate_folded` (the sole `RecordSubstrate` mint, routed through by
`KObject::record_of_held`) stores the substrate into its own brand's region exactly like
`alloc_object_folded`, so it carries no `unsafe` of its own beyond the `reattachable!`-generated
layout-invariance audit in `witnessed.rs`. The store-lands-in-its-own-brand round trip
(`alloc_substrate_folded_homes_a_record_substrate_in_its_own_brand`) frees nothing before its read —
a pure value assert — and runs under plain `cargo test`. The `alloc_carried_with` fold-brand
construction one level up (`record_retype_shares_substrate_across_producer_frame_free`, FROM's
`KObject::record_with_type` sharing a delivered `record` operand's substrate pointer across the
producer frame's free) also runs under plain `cargo test`: the combinator's by-construction dep
fold is pinned library-side (`alloc_with_folds_dep_reach_before_result_read` in the workgraph
slate), and the shared-substrate-across-producer-free shape stays pinned here by
`record_seam_pin_verb_shares_substrate_and_survives_producer_free` below. No separate minimal
test.

**`KFunction` captured-scope re-borrow** ([src/machine/core/kfunction.rs](../src/machine/core/kfunction.rs)) — every
closure invocation reads `KFunction::captured_scope`, now a bare field read of the stored
`&'a Scope<'a>` (re-anchored with the holder when it is read out of its region). The
escaped-closure shape — a closure returned out of its defining call and invoked after that frame
has returned — is pinned by `captured_per_call_value_survives_let_bind_and_call`, whose program
additionally dereferences a captured per-call value on the invocation.
The reading-the-captured-value tests pin the **delivered-carrier reach fold**
that keeps that defining region alive once the object channel is off the relocate seam: a
`let`-bound closure folds its carrier into the binding scope's reach-set, and a user-fn
closure argument folds into the per-call scope. Each reads a captured *outer* value after its
producing frame retires, so a lost region dangles under tree borrows. The multi-region case — a
`let`-bound list contributing *every* region a multi-region value reaches, which the single-frame
seam fold under-recorded — rides the mixed-cell list in the string re-home group below, where the
same fold answers the opposite way for its string cells.

- `captured_per_call_value_survives_let_bind_and_call`
- `closure_argument_stays_live_through_user_fn_call`

**Region-hosted string re-home at the substrate door** ([src/machine/model/values/kobject.rs](../src/machine/model/values/kobject.rs))
— a `KObject::KString` carries a `&'a str` bumped into the region the value lives in, and a string
cell's reach verdict is `Owned`: it names no region at all. That verdict is honest only because the
door re-bumps the bytes at the destination first (`section_cells` for a value cell, `alloc_dict` for
a key), so a cell that skipped the re-home would name no region while still pointing into a retiring
one — the exact shape no reach fold can rescue, and one no residence audit can catch either, since
the bump keeps no address table. Both tests build their strings inside per-call function regions,
bind the container in an outer scope so every producer frame retires, then read the bytes back: the
list test through the value cells, the dict test through a key lookup, which reads the stored key
bytes on the `str` compare. A door that shared the producer's pointer is a use-after-free here under
tree borrows. The only `unsafe` routed is the shared `retype` in `witnessed.rs`; the bump itself
carries none — a `&'a` into a region borrowed for `'a` needs no retype.

The list test's cells **interleave** strings with closures from distinct per-call regions, putting one
bind fold under both reach rules at once: a string cell's verdict names no region and is honest only
because of the re-home, while a closure cell rides a bare borrow into its defining region and is honest
only because the fold contributes *every* region a multi-region value reaches — the case the
single-frame seam fold under-recorded. Share the producer's string bump and the byte read dangles; drop
a closure's region and the captured read dangles.

The same rule binds a **bare** top-level string, where the door is the adoption seam rather than the
container's: a copying adoption claims the producer region's release (`retains_home` answers `false`
for a `KString`), so the relocation has to re-bump at the destination instead of pointer-copying the
producer's bump. `KObject::needs_destination_door` is the gate
`relocate_object_into` reads; keying the rebuild on the substrate
variants alone would leave exactly this shape pointer-copied under a release claim.

- `let_bound_list_of_call_produced_strings_and_closures_survives_every_producer_free`
- `let_bound_dict_with_call_produced_string_keys_survives_every_producer_free`
- `a_bound_bare_string_rebumps_at_its_destination`

**Region-hosted expression at the container door** ([src/machine/model/values/kobject.rs](../src/machine/model/values/kobject.rs))
— the expression peer of the group above, and the one check on the rule that lets
`KObject::KExpression` answer without a reach description at all: an expression cell's reach verdict
is `Owned`, `retains_home` answers `false`, and the expression door
(`RegionBrand::alloc_expression`) seals its cell with no member. All three are honest only because the node's `parts` run, its keyword text and its
structural cache live in the eternal-tier program storage that parsed them, which no relocation
releases. A node whose parts were bumped into the call region its producer ran in would satisfy
every one of those answers while pointing into a retiring region — and no address probe could catch
it, since the bump keeps no address table. The test produces its quotes inside per-call function
regions, binds the list in an outer scope so every producer frame retires, then walks the stored
`parts` on the read. The door is safe code throughout; tree borrows is the only check.

- `let_bound_list_of_call_produced_quotes_survives_every_producer_free`

**Bump-hosted substrate index re-home** ([src/machine/model/values/kobject.rs](../src/machine/model/values/kobject.rs))
— the peer of the group above one level up: not a *cell*, but the substrate's own **index metadata**,
which is bump-hosted and therefore has to be rebuilt at the destination exactly like the cells it
indexes. A record's index is the sorted name slice `alloc_record` bumps (the slice and every name's
bytes); a dict's is the `BumpMap` `alloc_dict` freezes over re-bumped keys. Both are read *before*
any cell is — a field lookup binary-searches the names and a key lookup hashes and byte-compares the
stored key — so an index that still pointed into a retiring producer is a use-after-free on the way
in, and a cell-only re-home would leave exactly that. The test builds both in one producer frame — a
record with unsorted field names, so the slice is genuinely reordered against the literal, holding a
dict whose keys were already re-bumped once into the producer, so the relocation must re-bump them
again rather than share — relocates the pair into a destination through the container-cell seam in a
single transfer, drops the producer frame, and only then walks the outer index and reaches the inner
one through it, looking every name and key up. The reach fold cannot rescue either shape: the index
is metadata, not a cell, so no run describes it, and the bump keeps no address table for an audit to
consult. Safe code; the only `unsafe` routed is the shared `retype` in `witnessed.rs`.

- `substrate_indexes_rehome_and_read_back_after_producer_free`

**Drop-free region death** ([src/machine/core/arena.rs](../src/machine/core/arena.rs))
— the closing claim of the shared per-region bump: every `Drop`-free value family lands there, so
region death for those bytes is chunk deallocation with **no per-slot destructor pass**. That is a
leak claim rather than a UB claim, and it is the one shape a bump cannot fail loudly on: a family
that quietly reintroduced an owning slot — a `Vec` spine, a `String` name, an `Rc` — still writes
and reads correctly, and the buffer simply never frees, because freeing a chunk does not visit what
the chunk holds. `Copy` is the static proxy that forbids it at every bump primitive; this test is
the dynamic check that the proxy is load-bearing in composition. It fills one frame region with all
five substrate shapes (list, dict, record, `Tagged`, `Wrapped`), each carrying a bumped string leaf
so the region holds re-homed bytes and index metadata as well as cells, then drops the frame with
nothing outside borrowing in. Miri's process-exit leak count is the assertion.

- `region_death_frees_every_drop_free_substrate_shape`

**Declaration-window sibling cell read from a sub-Dispatch**
([src/machine/model/types/typed_field_list.rs](../src/machine/model/types/typed_field_list.rs)) —
`rewrite_threaded_self_refs` seals each co-declared reference in a sigil field's body as a resident
cell in the *declarator's* scope region and bumps the rewritten body beside it. The field walker
reading those cells back runs inside the `:(LIST OF …)` sub-Dispatch — a step the declarator merely
parked on — and reads them through `read_resting`, which names **no pin at all**. So the claim under
test is entirely about ordering: the parked declarator's region outlives every step its own field
list spawned. Nothing the reader holds says so, and no other slate test pins a pin-less read whose
coverage belongs to a *parked* step rather than the reading one. The test declares a self-recursive
record whose sibling reference sits inside a record type nested in a sub-dispatched sigil — the one
position that reaches the walker's resolved-cell arm — and runs on to a constructed value, so the
sealed handle is used rather than merely elaborated.

- `declaration_window_sibling_cell_read_from_a_sub_dispatch_no_uaf`

**Retaining adoption's delivered re-home across retention** ([src/machine/core/scope.rs](../src/machine/core/scope.rs)) —
`Scope::adopt_carried` at the retaining seam consuming a *delivered* envelope. The verb's Pin arm is
a direct call into the library's fused mint-and-retain door (`Delivered::adopt_into`, pinned by the
workgraph slate's adoption tests); what is koan's own here is the **finalize seeding** the envelope
rides in on: the fold runs first, pinning the producer's residence
and the value's foreign reach into the consumer's region before the copy-free reattach fabricates the
consumer-lifetime borrow. This test finalizes an object at the Done boundary (mirroring production),
rides the retention hold across the producer shell's drop, adopts into an independent consumer scope,
then drops the hold and every other source handle before reading — the consumer's folded set is the
sole owner at the read, pinning the hold-to-fold handoff. Tree borrows catches a use-after-free if
the fold-before-reattach order, the host materialization, or the pin regresses.

- `retaining_adopt_object_rides_retention_across_producer_shell_drop`

**Dep envelopes held across a step's own open** ([src/machine/execute/run_loop.rs](../src/machine/execute/run_loop.rs))
— `run_step`'s consumer-step coverage is a plain `FrameCoverage` that absorbs each dep envelope's
own [`coverage`](../workgraph/src/witnessed/delivered.rs) (retained host ∪ reach). The
redundancy claim this is sound on: `dep_sources`' own `DepTerminal`s each hold the dep's *duplicated*
delivery envelope (owning the retention hold's `Rc` directly) across the whole step brand, so a
producer frame's liveness never rests on the step coverage alone. This end-to-end test drives five
real scheduler steps each producing into its own per-call frame, aggregates all five into one list
literal — a single consumer step opening five delivered deps at once, each cell rebuilt into the
aggregate's own region (`copy_held_from_carried`, so no producer materializes into the aggregate's
reach) — and confirms every producer arena is gone while the aggregate still reads correctly: a
use-after-free under tree borrows the moment the redundancy claim is wrong, and a lifetime leak
(the census reads live frames) the moment the fold re-pins a producer it copied out of. Its cells
**mix** the two escape verdicts the seam selects between — a region-pure scalar and a plain-data
record totally rebuilt by `copy_object_into` (escape with copy: no field borrows anything, so no run
of the rebuilt record names its producer) — so one run pins both against the same consumer open. The
width measurement that drove 100 identical producers is a value assert on the census and runs under
plain `cargo test` (`aggregate_of_call_results_releases_every_producer_frame`); identical producers
buy the audit nothing past the fifth. The only `unsafe` routed is the shared `retype` in
`witnessed.rs`.

- `aggregate_of_mixed_call_results_releases_every_producer_frame`

**Tail-hop argument adoption ordering (Lemma 2)** ([src/machine/core/scope.rs](../src/machine/core/scope.rs)) — a
tail call's loop-carried argument is delivered as its envelope (host = the retiring incarnation's
frame) and adopted through `Scope::adopt_for_binding`
([scope/reach.rs](../src/machine/core/scope/reach.rs)), whose copy verb
(`RegionEscape::Copy` run by `relocate_delivered` through the library's `transfer_into_placing`
fold) totally rebuilds the value into the adopting scope's own region: the rebuild's interior
borrows are minted into the adopter's composed reach before the product's `&'a` is fabricated,
and the release predicate is release-exact over the rebuilt product — a retiring host the product
no longer borrows is released with the retiring hold, so the retiring region frees strictly after
the adoption copy reads it. The test rebuilds an aggregate from the previous hop's own
carried value at every hop, so the spliced carrier genuinely pins the retiring region across the hop;
tree borrows catches a use-after-free if the free ever reorders before the adoption read. The
record-embedding twin of this same adoption path — each hop threading a fresh `{acc = …}` record
argument through `THREAD`'s `it`-bind, whose plain-data rebuild releases the retiring incarnation
so the region count stays depth-independent (`O(1)`) — asserts a loud `region_metrics()` peak
bound with no Miri-only failure mode, and runs under plain `cargo test`
(`tail_recursive_record_thread_stays_o1_in_regions`).

- `loop_carried_aggregate_survives_tail_hop_adoption`

**Resting splice cell read across a tail hop** ([src/machine/execute/run_loop.rs](../src/machine/execute/run_loop.rs)) —
a spliced sub-result rests as a pin-less `Sealed` cell in the *dispatching* step's own region
(`Scope::rest_delivered`), and the step that adopts it is a **later incarnation of the same slot**,
running against a freshly minted cart whose ancestor chain does not reach the retiring one. What spans
the hop is the run loop's TCO handoff hold, absorbed into the step's coverage and named as the pin for
`SchedulerView::lift_spliced`'s `Sealed::open_at` + `Opened::lift_out`. Tree borrows catches a
use-after-free if that ordering ever breaks, and one hop is the whole shape — the slate test runs a
depth-1 loop and reads its result back. That the retention is also *per-iteration* — the peak
live-region count flat in the loop's depth, where pins chained forward would grow it one region per
hop — is a loud `region_metrics()` comparison across depths 3 and 11 with no Miri-only failure mode,
and runs under plain `cargo test` (`a_splicing_tail_loop_holds_no_region_per_iteration`).

- `a_spliced_cell_survives_its_tail_hop`

**MATCH / TRY-WITH branch frames inside TCO position** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) —
MATCH and TRY build their per-branch frame and seed the `it` bind through
`CallFrame::with_scope`: the matched value, deep-cloned at the caller lifetime, is relocated into
the opened child scope's own region through the substrate (rebuilt at the destination brand, which
is where the caller lifetime is dropped) and bound; the `FrameStorage` ancestor chain keeps the
call-site region alive across TCO replace when a user-fn recurses through a `Tagged` parameter.
The test drives both doors in one program — a MATCH on a `Tagged` scrutinee nested inside a TRY
whose `Ok -> it` catch path tail-calls back through the enclosing user-fn — so the branch frame
chain, the `it`-bind seed relocation, and the framed TCO replace are all exercised together.

- `try_inside_tco_position_preserves_frame_chain`

**`KFunction::invoke` per-call frame re-anchor** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — the
seed bind routed through `CallFrame::with_scope`: the deep-cloned argument record is relocated into the
opened child scope's own region through the substrate (rebuilt at the destination brand, which is
where the caller lifetime is dropped) and each parameter bound, while the scope rides the `for<'b>`
brand the open confines. Witnessed by the `Rc<CallFrame>`
moved into `BodyResult::Tail`. Exercised end-to-end by every user-fn invocation a scheduler-driving
slate test makes: repeated-call reclamation, type-op dispatch through a functor-call's per-call
scope, and `MODULE_TYPE_OF` lift-out. The repeated-call growth bound
(`repeated_user_fn_calls_do_not_grow_run_root_per_call`) is a value assert on region metrics and
runs under plain `cargo test`; its process-exit leak residue is covered by the aggregate census
test below. No separate minimal test.

**Stored reference-carrier re-anchor** ([src/machine/core/ref_carriers.rs](../src/machine/core/ref_carriers.rs)) — every
holder stores a captured / defining / parent scope as a plain `&'a Scope<'a>` (`Module::child_scope`,
`KFunction::captured`, `Scope::outer` / `root`) and re-anchors it **with
the holder as a whole** when the holder is read out of its region (the `Region::alloc_resident` retype in
`witnessed.rs`), so the accessors are bare field reads and ref_carriers.rs carries no `unsafe` of its own.
The construction-time reference is built at `'a` by plain coercion (a same-region child) or at the
construction door's generative brand (a per-call frame child, `build_frame_child_witnessed`) — there is
no construction-time re-anchor verb. The shape is pinned library-side by the workgraph slate's
born-door group over an invariant holder that embeds a foreign-region parent and takes an
interior-mutable write after the store (`the_born_with_door_embeds_a_parent_from_another_region`,
`the_returned_node_accepts_a_write_at_the_callers_lifetime`, and their siblings); koan's
`Module::child_scope` / `KFunction::captured_scope` behavioral round trips run under plain
`cargo test`, and every `Scope::outer()` / `ancestors()` walk reads the field end-to-end. No
separate minimal test here.

**`NodeScope::YokedChild` lifetime fabrication** ([src/machine/execute/nodes.rs](../src/machine/execute/nodes.rs))
— a cart-ancestor block scope evicted off a lifetime-free scheduler node (`NodeScope::YokedChild`) is
stored as a `SealedExtern<ScopeRefFamily>` through the safe `SealedExtern::erase`
(`erase_to_static::<ScopeRefFamily>`) and opened at the read boundary through the `for<'b>`
`SealedExtern::open` — the brand confined to the read, witnessed by the slot's frame `Rc`, sound because
the cart's `outer_frame` chain pins the ancestor region. This is the second lifetime-free scope carrier
(alongside `CallFrame`). The erase → open round trip — including a sibling store into the region while
the opened reference is live — is pinned library-side
(`sealed_extern_open_externally_witnessed`, `an_erased_node_opens_and_survives_a_sibling_store_inside_the_open`
in the workgraph slate); every scheduler-driving slate test below exercises the production carrier
end-to-end. No separate minimal test here.

**`NodeScope::YokedChild` open — workload read boundary** ([src/machine/execute/dispatch/ctx.rs](../src/machine/execute/dispatch/ctx.rs))
— the `carrier.open(frame, f)` call in the `with_node_scope` helper is the **sole** production
open of a `YokedChild` carrier: it materializes the executing slot's scope from its raw
`NodeScope` handle (the scheduler core hands the handle back but no longer interprets it), passing the
slot's cart `Rc` as the witness to the `for<'b>` `SealedExtern::open` — a **safe** call, no `unsafe`
here. The decide-phase read (`current_scope`, via `SchedulerView`), the Done-boundary post-step read
([src/machine/execute/run_loop.rs](../src/machine/execute/run_loop.rs)), and the `OwnScope`
re-dispatch (`KoanRuntime::dispatch_in_own_scope` in
[src/machine/execute/runtime/submit.rs](../src/machine/execute/runtime/submit.rs), which clones the
cart `Rc` locally and routes this helper) all funnel through it — none carries an `unsafe` of its own.
It runs the same transmute the workgraph slate's `SealedExtern` opens pin, and every
scheduler-driving slate test exercises it end-to-end. No separate minimal test.

The `retype` primitive (`Erased<T>` / `Witnessed<T, W>`) and the `ReturnContract`
re-attach it backs at the Done boundary are audited in the `workgraph` crate's own
slate — [workgraph/observe/miri_slate.md](../workgraph/observe/miri_slate.md) — since
their tests live in that crate's lib test binary, a separate `cargo test` target from
koan's. `CarriedFamily`'s `unsafe impl Reattachable`
([src/machine/model/values/carried.rs](../src/machine/model/values/carried.rs)) and this
embedder's `HasRegionHandle` destination operands
([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — over the library's
`RegionSet<FrameStorage>` that `FrameSet` aliases (`FrameStorage` = `RegionHost`, whose `PinsRegion`
lives library-side) — are the Koan-side instantiations that primitive
routes for; `RegionSet::union`'s antichain logic (union with `outer`-chain subsumption) is pinned by
the `frameset_*` / `pins_region_walks_outer_chain` unit tests in
[arena/tests.rs](../src/machine/core/arena/tests.rs), which run under plain `cargo test`.

**`ContinuationFamily` continuation erasure** ([src/machine/execute/outcome.rs](../src/machine/execute/outcome.rs))
— the continuation generalizes the contract discipline from a `ReturnContract` enum to the whole
`NodeContinuation` (`Box<dyn FnOnce>`). It is koan's only carrier family on the **owned tier**: a
boxed `FnOnce` owns its captures, so the family takes the `droppable` `reattachable!` arm (no
`DropFree`) and rests as `SealedPinned<ContinuationFamily, Rc<SlotFrame>>`, sealed against the slot's
anchor at the scheduler's install door and opened once per step by that tier's single consuming verb.
Both directions route the shared `retype`: `erase` forgets the captured `'run` for storage on a
lifetime-free node, and `open` recovers a step brand `'b` witnessed by the seal's own bundled anchor
`Rc` (which pins the captures' home — the run region or a strict ancestor of the cart). The
distinguishing layouts — the retype over a **fat pointer** (data + vtable), consumed by value and
invoked inside the brand, and the drop of a parked slot that was never opened — are pinned
library-side (`sealed_extern_open_invokes_a_fat_pointer_continuation`,
`parked_continuation_drops_under_its_own_pin`, and
`parked_continuation_opens_and_runs_after_its_handles_drop` in the workgraph slate). The open call
site in
[src/machine/execute/run_loop.rs](../src/machine/execute/run_loop.rs) (`run_step`) runs the same
transmute end-to-end every step, exercised by every scheduler-driving slate test. No separate
minimal test here.

The run-loop step-tail open (`run_step`, opening the sealed continuation and the active-scope
operand together at one generative brand) and the doctest fixture markers backing the
`compile_fail` soundness guards are audited in
[workgraph/observe/miri_slate.md](../workgraph/observe/miri_slate.md) alongside the `retype` group they
route through — [src/machine/execute/run_loop.rs](../src/machine/execute/run_loop.rs)'s and
`finalize.rs`'s call sites carry no `unsafe` of their own.

**`Module` interior mutation under a live `&'a Module`** ([src/machine/model/values/module.rs](../src/machine/model/values/module.rs)) — `Module`
mutates a `RefCell<HashMap>` (`type_members` / `slot_type_tags`) while a `&'a Module<'a>` is
live — the opaque-ascription shape. (The scope re-anchor itself is the stored scope-pointer group
above; the carrier stores a `&'a Scope<'a>`.) The interior-write-through-the-re-anchored-holder
shape is pinned library-side (`the_returned_node_accepts_a_write_at_the_callers_lifetime` and
`invariant_same_brand_mutation` in the workgraph slate); the koan `RefCell` round trips
(`module_type_members_refcell_mutation_with_held_module_ref` and its `slot_type_tags` twin) and the
end-to-end `opaque_ascription_re_binds_do_not_alias_unsoundly` run under plain `cargo test`. No
separate minimal test here.

**MODULE body Combine continuation** ([src/machine/model/values/module.rs](../src/machine/model/values/module.rs)) — the
MODULE body schedules a `Combine` whose `finish` closure captures the child
scope and runs on the outer scheduler's main loop after every body statement
terminalizes. Runs the same stored scope-pointer re-anchor the workgraph slate's born-door group
pins (see the stored reference-carrier group above) with none of its
own, exercised end-to-end by every scheduler-driving slate test; its `module_body_dispatch_does_not_dangle`
program runs under plain `cargo test`. No separate minimal test.

**`Scheduler::replace` / `NodeStore::reinstall` slot re-anchor** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) —
the Replace arm stores the slot's scope as a payload-less `NodeScope::Yoked` marker re-projected
from the frame cart (no fabricated `&'a` persists), so the `Rc<CallFrame>` witness in `Node.frame`
remains the sole liveness root for the re-installed slot's scope.
Exercised by the dispatch-time parking shapes that reinstall through this entry
point (and transitively by user-fn TCO; that path is covered by the MATCH-on-
`Tagged` recursion test above). One batch-submitted program drives both: `LET y = z`
forward-splices a bare name whose producer has not run yet, and `LET out = (DOUBLE y)` parks a FN
call on that same binding and replays it on the wake — the parked slot's scope must stay valid across
both the wake and the re-dispatch.

- `park_and_replay_minimal_program_for_miri`

**`Carried` slot read + dep re-anchor — pinned `open_with`** ([workgraph/src/scheduler/node_store.rs](../workgraph/src/scheduler/node_store.rs))
— the scheduler stores a finalized terminal as a `Witnessed<W::Value, Carrier<W::Frame>>` — the
reference-only carrier, pinning nothing — beside the retention hold finalize seeds, and
`read_result_with` re-anchors under that retained frame owner (`open_with`); a slot with no retained
owner (a drained root re-homed into the run region) is externally pinned, so its read opens under
the empty pin. The consumer-pull dep terminals travel as delivery envelopes — `dep_delivered`
duplicates the slot's envelope per consumer, opened in the consumer `dest` region at `'b`.
`node_store.rs`'s own residual `unsafe` is
only the test-family `Reattachable` markers. Exercised end-to-end by every scheduler-driving program;
the listed test pins the hardest shape — a tail-chain return-type **coarsening** re-homed in the
contract's scope, re-read after the run drains the root into the run region.

- `tail_call_stamps_result_against_first_callers_return_contract`

**`Carried` relocation + escaping-value retention** ([src/machine/execute/lift.rs](../src/machine/execute/lift.rs))
— `copy_carried` structurally copies a `Carried` into the consumer `dest` region at the brand the
step `open` supplies (a safe alloc): under the per-value verb `relocate_object_into`, a value
keeping region storage behind (`needs_destination_door` — a substrate carrier, or a bare `KString`)
is totally rebuilt so it lands at `dest`, and a
closure / `KFuture` / `Module` rides a *bare* borrow into its defining region, never copied. That
surviving borrow outlives the producer's frame
only because the witnessed transfer mints the value's reach — the borrowed region among it — into
the destination's arena, whose composition retains it for the consumer region's life. Safe code; pinned
because tree borrows catches a regression in the retention discipline that would dangle an escaped
closure / module past its per-call frame's drop. The closure shape rides the `KFunction`
captured-scope group above; the tests below pin the **module** shape — a functor-minted module
surviving the run that built it, and a **transparent-ascription view**, the one value shape whose
residence and the scope it borrows are different regions (the view is re-tagged into the ascribing
call's own region while its child scope stays where the source module put it). A borrow leaf is never
rebuilt, so a relocation carrying one out of a dying frame must keep the region it *lives* in, not the
one it borrows; a release claim derived from the child scope frees the storage the returned value
points at, which only tree borrows observes — a normal build reads the freed bytes back intact.

- `functor_application_is_generative`
- `a_returned_transparent_view_keeps_the_region_it_was_minted_in`

**Record escape seam — cost-driven copy vs pin** ([src/machine/execute/lift.rs](../src/machine/execute/lift.rs))
— two distinct seams relocate a top-level `Record` out of a dying producer, each pinned here. The
**container-cell** seam (`cell_still_borrows`, Ruling 4: fresh containers stay self-contained,
never a pin) picks each crossing cell's release: the producer frees when the retention predicate
reads no surviving borrow leaf into the cell's own producer host off the rebuilt cell's stored
reach (the cell is totally rebuilt via `copy_held_from_carried`), and the producer materializes
into the aggregate's reach and stays pinned when it does. One unit test mirrors
`dispatch::literal::fold_cells`'s exact aggregate loop
(`cell_still_borrows` + `transfer_into_placing` + `copy_held_from_carried`) directly for five
independent producers, each record cell embedding a closure captured in that same producer (every
producer pinned; drops every producer first, then reads every closure's captured scope back) —
wrongly releasing a still-borrowing record dangles under tree borrows; wrongly pinning leaks. The
plain-data twin (`plain_record_cells_select_released_and_survive_every_producer_free`, every
producer released) runs under plain `cargo test`: the retention verdict is derived from the rebuilt
cell's own stored reach, so a release verdict that disagrees with what the rebuild left behind is
unrepresentable, its rebuilt plain-data cells hold no borrow leaf that could dangle, and the
release direction stays Miri-audited end-to-end by the mixed aggregate census test above, whose
record cells ride this same `fold_cells` seam through the real scheduler and parser.

The **value-level** escape seam (the fused `relocate_seam`: the `copy_or_pin` cost chooser
([kobject.rs](../src/machine/model/values/kobject.rs)) at `relocate_terminal` and the literal park
finish) picks the whole record's `RegionEscape` in O(1) from its memoized copy cost
and borrows-home bit: a **copy** (totally rebuilt at the destination, the retention claim read off
the rebuilt product releasing the producer) when a priceable plain record is a small fraction of
the host's allocated total, and a **pin** (the envelope's pins claimed whole) when a leaf
borrows home — the
record's region-resident substrate rides **shared** (a pointer-copy, never rebuilt), covered by the
producer reach the pin mints into the destination. The released-copy shape runs end-to-end through
the real scheduler and parser in the mixed aggregate census above, whose record cells corroborate the
seam is wired to real per-call producer frames; the minimal-shape twin here drives the
cost-chooser-selected pin for five independent home-borrowing records through `relocate_seam`
itself (asserts `Pin`, drops every producer, then reads each closure's captured scope back through
the shared substrate), so a
regression that failed to mint the kept host — or rebuilt instead of sharing — dangles here. Both
verbs are thus UB-audited at the seam under the default cost chooser. The `unsafe` routed is the
shared `retype` in `witnessed.rs`; `lift.rs` carries none of its own (see the file's stale-group
whitelist entry).

- `closure_embedding_record_cells_select_copied_and_pin_every_producer`
- `record_seam_pin_verb_shares_substrate_and_survives_producer_free`

## Adding tests to the slate

Add a test to the slate when a new unsafe site lands — a transmute, raw-pointer
round-trip, interior-mutation pattern under a live shared borrow, or a cycle
shape that storage-side reasoning can't rule out. Tests are minimal-shape
mirrors of the unsafe operation, not end-to-end feature tests; they fail when
Miri reports UB or a leak, not on values.

When you add or remove a slate test, update the list above (the section
structure mirrors the unsafe-site groupings, so a new test lands under the
group it pins down — or under a new group if it's a new shape) and re-run the
slate to confirm the line count matches.

## Recent full-slate run durations

The five most-recent full-slate runs, newest first. The Miri skill appends a
new entry on every full-slate run and trims to five so this list stays bounded.
Use the most-recent entry as the baseline expectation when scheduling a run.

<!-- slate-durations:start -->
- 2026-08-04: 962s — 22 tests, 0 leaks, 0 UB
- 2026-08-04: 1047s — 24 tests, 0 leaks, 0 UB
- 2026-08-04: 1014s — 25 tests, 0 leaks, 0 UB
- 2026-08-04: 971s — 25 tests, 0 leaks, 0 UB
- 2026-08-03: 1028s — 27 tests, 0 leaks, 0 UB
<!-- slate-durations:end -->
