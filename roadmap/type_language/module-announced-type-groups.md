# Module bodies announce type groups

Retire `RECURSIVE TYPES`; module bodies pre-announce their top-level type
declarations, so any module hosts mutually-recursive nominals.

**Problem.** Mutually-recursive nominal types require the dedicated
`RECURSIVE TYPES <Name> = (body)` block
([`recursive_types.rs`](../../src/builtins/recursive_types.rs)): its shallow pre-scan
(`discover_members`) announces member names before their declarations elaborate, which
is what lets cyclic sibling references resolve instead of mutually parking. The block
duplicates module-shaped machinery for that one feature — module bodies already admit
type declarations (mirrored into `Module::type_members`) and already park-resolve
*non-cyclic* forward references on the outer scheduler, but a cycle declared in a
`MODULE` body deadlocks. The block's body is restricted to `UNION`/`NEWTYPE`
statements, and its group name binds a `TypeNode::Group` handle — a type-position
binding that admits no values, with inert predicate arms
([`ktype_predicates.rs`](../../src/machine/model/types/ktype_predicates.rs)).

**Acceptance criteria.**

- Top-level `NEWTYPE`/`UNION` statements in a module body are pre-announced: mutually
  visible and order-independent, so a mutually-recursive group declared in a plain
  `MODULE` body seals correctly. Nested or computed declarations are not announced and
  keep ordinary dataflow order. `GROUP` inherits the behavior (a group is a module).
- An announced member still unfilled when the module body completes surfaces a typed
  `KError`, never a hang or panic.
- `RECURSIVE TYPES` is removed: the builtin is gone, and the group-binding type
  machinery (the registry's `Group` node and `TAG_RECURSIVE_GROUP`) is deleted.
- Announcement does not perturb identity: two unrelated types co-declared in one
  module body keep decoupled digests and unify with their standalone twins (a test
  pins this against the computed-SCC identity rule).
- A mutually-recursive group at program top level requires a module wrapper — pinned
  as intended surface, documented in the tutorial.
- The tutorial and design docs describe the module surface only:
  [`tutorial/08-newtypes.md`](../../tutorial/08-newtypes.md)'s `Listy` example becomes
  a `MODULE` + `USING`, and
  [`user-types.md`](../../design/typing/user-types.md) /
  [`modules.md`](../../design/typing/modules.md) carry the announcement rule.
- The module, recursive-group, union, and functor suites are green.

**Directions.**

- *Announcement by shallow pre-scan of top-level statements — decided.* The block's
  `discover_members` scan hoists to module-body entry unchanged in spirit: leading
  `NEWTYPE`/`UNION` keywords at the body's top level announce; nothing else does. The
  boundary is the same one the block draws today.
- *Depends on computed-SCC identity — decided.* Announcing a whole module body would
  co-declare unrelated types together, and only computed-SCC identity keeps that
  identity-neutral (a co-declared member that references no sibling digests
  independently). That identity model shipped with interned-type-content, so announced
  membership no longer couples identities and the relocation is unblocked.
- *Top-level cycles take the wrapper — decided.* Announcement stays a module property
  rather than a global scan rule; the program body is not special-cased.

## Dependencies

Retiring the `Group` node resolves the "reserved for value-language cycle
construction" question recorded in
[Constructing circular values](circular-value-construction.md) — resolved as retired,
not consumed.

**Requires:**

- [USING surfaces module type members](using-type-members.md) — the migration path for
  members that today mirror flat into the enclosing scope.

**Unblocks:** none tracked yet.
