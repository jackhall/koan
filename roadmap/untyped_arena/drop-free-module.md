# Drop-free `Module`

Moves the module family out of its typed cell into the region bump — the
storage model of
[design/value-substrates.md § Untyped arenas](../../design/value-substrates.md#untyped-arenas-the-drop-free-end-state).

**Problem.** A `Module` owns its `path: String` and two
`RefCell<HashMap<String, KType>>` member maps
([src/machine/model/values/module.rs](../../src/machine/model/values/module.rs)).
The maps are written *after* the value is allocated — opaque ascription
installs entries post-alloc — which is why they are live `HashMap`s rather
than a build-once index, and they are what hold the family in a typed cell.

**Acceptance criteria.**

- `path` is a bumped `&str` and the member maps are bump-hosted.
- `Module` is `Drop`-free and stored through the bump doors; its typed cell and
  `Stored` impl are deleted.
- The Miri leak slate covers module-heavy programs, opaque and transparent
  ascription both.

**Directions.**

- *Map substrate — open.* Construction already ends at a seal point
  (`seal_self_sig` runs after the maps are populated), so the preferred shape
  is gathering entries in a builder and sealing a build-once `BumpMap`; a
  `Cell`-rooted bump-hosted structure is the fallback if construction cannot
  be restructured. Recommended: builder-then-seal.

## Dependencies

**Requires:**


**Unblocks:**

- [Frame-owned scopes retire the typed cells](frame-owned-scopes.md)
