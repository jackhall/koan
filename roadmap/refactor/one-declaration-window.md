# One declaration-window representation

A group of co-declared nominal types elaborates against one window type, not two behind a
forwarding enum.

**Problem.** There are two declaration-window representations with the same interface, and two enums
whose whole content is forwarding between them:

- `AnnouncedWindow`
  ([declaration_window.rs](../../src/machine/model/types/declaration_window.rs)) — the ambient
  window a module body's pre-announced declarations elaborate against. Every field is a bumped
  `Copy` run or a `Cell` of one, because it sits inline in `ScopeKind::Module` and a scope must stay
  Drop-free.
- `RecursiveGroupWindow`
  ([recursive_group_window.rs](../../src/machine/model/types/recursive_group_window.rs)) — the
  declarator-local window. `RefCell<Vec<_>>` fields, because it carries `KKind::TypeConstructor`
  schemas and grows by threaded discovery, neither of which the ambient window needs.

`WindowView` is twelve methods of two-arm dispatch over the pair, and `DeclWindow` is two more. The
identity computation is already shared — both seal through the pure `seal_group` — so what the split
buys is the storage difference alone, at 1,253 lines across the two files plus the shims. The
constraint that forced it is real: the ambient window must cost region teardown nothing. But
`AnnouncedWindow::fill` already shows growth is expressible under that constraint — it replaces the
whole fills run on each fill rather than mutating a cell — so the constraint does not by itself
require two types.

**Acceptance criteria.**

- One window type serves both the ambient and the declarator-local role: it is Drop-free and
  bump-hosted, carries `NewType` and `TypeConstructor` schemas alike, and grows by threaded
  discovery.
- `WindowView` and `DeclWindow` are gone, or reduced to a borrow that forwards nothing.
- A module body's window still sits inline in `ScopeKind::Module` with no allocation and no `Rc` of
  its own, and the sections-are-Drop-free invariant
  ([memory-model.md](../../design/memory-model.md)) holds unchanged — frame death stays `O(1)`.
- A generative `:|` mint, a standalone `NEWTYPE`, a `UNION` over its variants, and a
  mutually-recursive group in a `MODULE` body all open, fill and seal through that one type, and
  every identity the current suite pins is unchanged.
- `seal_group` takes one input shape rather than the two `SealMemberInput` / `SealBinderInput`
  assemblies its two callers build today.

**Directions.**

- *Which representation survives — decided.* The bumped `Copy` one. The Drop-free requirement is
  non-negotiable and the growable one cannot be made to satisfy it, while the bumped one already
  demonstrates growth by run replacement.
- *Where a declarator-local window is hosted — open.* A standalone declaration has no module scope
  to sit inside. Candidates: the declaring step's scratch arena, or the declarator's own frame
  region. Recommended: the frame region — a window outlives the step that opens it whenever a
  consumer parks on the seal, which the step scratch does not survive
  ([step scratch is reset at every drain pop](../../design/per-node-memory.md)).
- *Growth cost — decided.* Run replacement per fill, as `AnnouncedWindow::fill` does. Member counts
  are a handful; the copies are free and the alternative is a per-member cell that cannot be bumped.
- *`TypeConstructor` schemas in a bumped run — open.* `RelativeSchema::TypeConstructor` holds a
  `TypeMemberMap` and a `Vec<TypeSymbol>`, neither Drop-free. Candidates: bump both as runs, or keep
  the schema owned behind a handle the window stores. Recommended: bump both — the map is small and
  keyed by `Copy` symbols, so a run with linear probing matches how `Record` already backs a field
  schema.

## Dependencies

**Requires:** none — a leaf refactor over shipped machinery.

**Unblocks:** none tracked yet.
