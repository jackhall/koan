# A top-level `memory` module

Koan's instantiation of the region substrate, and every substrate name Koan spells, live in one
top-level module, `src/memory/`, that the rest of the crate builds on.

**Problem.** The payload-generic region engine is [workgraph's witnessed
module](../../workgraph/design/witnessed-memory.md); Koan's instantiation of it is spread across two
unrelated parents. Under `machine::core` sit the storage profile, `KoanRegion`, `CallFrame`, program
storage and the step allocator ([arena.rs](../../src/memory/region.rs),
[arena/frame.rs](../../src/memory/frame.rs),
[arena/step_allocator.rs](../../src/machine/execute/step.rs)), the sealed and
delivered carrier aliases ([carrier_witness.rs](../../src/memory/substrate.rs)) and the
reference families ([ref_carriers.rs](../../src/machine/core/scope.rs)). Under
`machine::model::values` sit the value-channel cells `Held` and `Carried`
([carried.rs](../../src/machine/model/values/cell.rs)), the container substrates
([container_substrate.rs](../../src/memory/container_substrate.rs)) and the rehoming
door ([rehomed.rs](../../src/memory/rehomed.rs)). Nothing names the set: a reader
looking for "where a value lives and how long" walks [memory-model.md](../../design/memory-model.md),
[value-substrates.md](../../design/value-substrates.md) and
[per-call-region/](../../design/per-call-region/README.md) to find files in two directories that
each also hold unrelated machinery — `machine::core` hosts scopes, bindings, functions and errors
beside the arena; `values` hosts coercion, equality and hashing beside the cells.

The library's own names leak past that set. Fifty-eight files outside it import
`workgraph::witnessed` directly — `Delivered<T, CarrierWitness, FrameStorage>`,
`RegionHandle<'a, KoanStorageProfile>`, `FoldedPlacement<'b, KoanStorageProfile>`, `BumpVec`,
`BumpAllocator` — each spelling Koan's witness, owner or profile again at the use site; the
bump-backed table constructor `bump_table` sits in [bindings.rs](../../src/machine/core/bindings.rs)
and names `hashbrown` there. Swapping the substrate touches every one of those sites.

The mixed hosting also hides the dependency direction. `parse` already bumps its output into program
storage (`ProgramBrand`, `program_storage`), so the parser depends on the region layer, yet it reaches
it through `machine::core`, which makes `parse → machine` look like a dependency on the interpreter.
And the cells carry queries that belong to the registries: `Held::ktype` takes a `TypeRegistry`,
`Held::summarize` a `RunRegistries`, so the cell file imports `KKind`, `TypeRegistry` and
`display_label` to answer questions the registry owns.

**Acceptance criteria.**

- `src/memory/` is a top-level module holding the storage profile, `KoanRegion`, the region brands,
  `CallFrame`, program storage and its brand, the carrier aliases, the container substrates, the
  rehoming door, the `Held` / `Carried` cells and the bump-backed table constructor. Each moves with
  its tests; `machine::core` and `values` keep none of them, and re-export none of them.
- Every substrate name Koan spells is a `memory` item: `memory` defines one Koan-bound alias per
  `witnessed` generic (`Delivered<T>`, `Sealed<'h, T>`, `Opened<'b, T>`, `Witnessed<T>`,
  `Retained<T>`, `Unhosted<T>`, `RegionHandle<'a>`, `RegionHandleFamily`, `FoldedPlacement<'b>`,
  `Sectioned<'a, K>`, `StepContext`, …) and re-exports the bump types (`BumpAllocator`, `BumpVec`,
  `BumpBackedMap`) and the reattach vocabulary. Outside `src/memory/`, no file imports `workgraph`,
  `crate::witnessed`, `hashbrown` or `allocator_api2`, and `lib.rs` re-exports no `witnessed`.
- The step-brand layer is one file, `machine::execute::step`, holding `StepCarried`,
  `StepAllocator` and the two `alloc_*_witnessed` brand doors; `memory` defines no door that
  returns a step-branded value.
- Each reference family sits beside its type (`ScopeRefFamily`, `RegionScopeFamily` with `Scope`;
  `BindingsReferenceFamily` with `Bindings`; `ModuleRefFamily` with `Module`); the registration
  bundles `OverloadSeal` and `GroupSeal` stay in `machine::core`.
- A cell's type tag and rendering are registry methods — `TypeRegistry::ktype_of`,
  `TypeRegistry::ktype_of_carried`, `RunRegistries::held_summary`, `RunRegistries::carried_summary`
  — and `Held` / `Carried` define no method that takes a registry.
- `parse` reaches program storage through `memory`, not through `machine::core`.
- `memory` imports nothing from `machine::execute`, and nothing from `machine::model::types` beyond
  the `KType` handle.
- Every drop-free assertion, `needs_drop` check and Miri slate test that guarded a moved file guards
  it at its new path; [TEST.md](../../TEST.md)'s slate list and the `observe/miri_slate.md` log name
  the new paths, and the slate passes.
- [memory-model.md](../../design/memory-model.md),
  [value-substrates.md](../../design/value-substrates.md) and
  [per-call-region/](../../design/per-call-region/README.md) point at `src/memory/` for every
  substrate concept they name, and the README's source layout lists the module.

**Directions.**

- *Module, not crate — decided.* `memory` is a module inside Koan. The crate boundary that enforces
  the substrate direction already exists at `koan → workgraph → cellgraph`; Koan's instantiation
  names Koan types and belongs above it.
- *How `Held` and `Carried` name their payload — decided.* `memory` names `KObject` concretely and
  takes the back-edges the cells need: `machine::core::{scope, scope_id, kfunction}` and
  `machine::model::{values::kobject, values::kkey, labels, ast, operators}`, listed in the module
  doc. Parameterising the cells over a payload family, which would invert those edges, is not part
  of this item.
- *Substrate names — decided.* One alias per library generic with Koan's witness, owner and
  profile bound; a re-export where the library type takes no Koan parameter. A site that needs a
  parameter the alias does not bind is a design question, not a second alias.
- *Cell queries — decided.* They invert onto the registries: types live in the registry, and a
  `types → memory` edge is the right direction.
- *Step brand — decided.* `StepCarried`'s only exit is `pub(super)` inside `execute`, so the
  allocator and the brand doors that mint it live there, not in `memory`; the two `*_witnessed`
  doors are an inherent `impl RegionBrand` block in that file. This file and `memory` are the two
  seams [adopt-cellgraph](../../workgraph/roadmap/adopt-cellgraph.md) rewrites.
- *`Scope` interface — deferred.* `CallFrame` and the storage profile name `Scope` concretely. The
  trait that lets a lightweight per-call scope and the lexical scope both fill that slot is written
  with the second implementation
  ([slot-shaped per-call scopes](../reduce_allocs/slot-shaped-per-call-scopes.md)), not here.
- *`bindings` and `scope` stay in `machine::core` — decided.* They are scope semantics over the
  region, not the region; they move only if the `Scope` interface pulls them.

## Dependencies

First of the three-step reshuffle this item opens: `memory`, then the parse consolidation, then
the type-lattice rewrite. It also narrows the Koan-side surface
[adopt-cellgraph](../../workgraph/roadmap/adopt-cellgraph.md) rewrites to `memory` and
`execute::step`, without being a prerequisite of it.

**Requires:** none — a foundation move over shipped machinery.

**Unblocks:**

- [Consolidate `parse`](consolidate-parse.md) — the parser's program-storage import lands on
  `memory` rather than `machine::core`.
