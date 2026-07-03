# A compile-enforced boundary for the substrate

Begin the extraction itself: put the library-designated code — the witnessed
substrate (including the generic region engine) and the scheduler — behind
one boundary that **cannot** name a Koan type, per the boundary table in
[design/scheduler-library.md](../../design/scheduler-library.md).

**Problem.** The library-designated code already has the no-Koan-imports
property — `src/witnessed.rs` (with `witnessed/region.rs`, the generic
`Region<W: StorageProfile>` engine) and `src/scheduler/` import nothing from
`machine`/`builtins`/`parse` — but nothing enforces it: they are ordinary
sibling modules of the code that embeds them, and one library-generic item
already sits on the wrong side (`unsafe impl<W: StorageProfile> Witness for
Region<W>` lives in `src/machine/core/arena.rs:66`, next to the Koan profile,
rather than with the types it describes). A stray `use crate::machine::…`
added to scheduler code tomorrow would compile without complaint and silently
re-couple the halves the extraction needs separate.

**Acceptance criteria.**

- The witnessed substrate (including the region engine) and the scheduler
  live behind one boundary whose code cannot reference Koan types, enforced
  at compile time.
- Library-generic impls stranded on the Koan side (at minimum the
  `Witness for Region<W>` blanket impl in `arena.rs`) live inside the
  boundary; `machine/core/arena.rs` keeps only the Koan profile
  (`KoanStorageProfile`, `KoanRegion`, brand/type families, `FrameSet`,
  `CallFrame`).
- The boundary's public surface is the enumerated set of items the embedder
  uses (facade re-exports; no `pub` leakage of internals).
- No behavioral change: `cargo test` green and the Miri audit slate clean
  (this is a memory-substrate move).

**Directions.**

- *Boundary mechanism — open.* (a) a workspace sub-crate (compile-enforced by
  the dependency direction: the substrate crate cannot depend on the koan
  crate); (b) one top-level module plus an automated import-hygiene check.
  Recommended: (a), matching the repo's compile-enforcement preference;
  fall back to (b) only if the workspace split fights tooling
  (modgraph/doclinks paths, test layout).
- *Crate/module name — open.* The design doc says "the scheduler library";
  pick a working name (e.g. `substrate`) and record it there when this ships.
- *`FrameSet` stays put — decided.* Reach-set opacity (guarantee 2) is its
  own later item; this item moves no reach semantics.

## Dependencies

Coordinate with the
[`Await` envelope builder](await-envelope-builder.md), which adds code to the
same scheduler tree — pure file-motion conflicts, no logical dependency.

**Requires:** none — the property it enforces already holds.

**Unblocks:** none tracked yet — regions-wholesale ownership and the opaque
reach set land inside this boundary.
