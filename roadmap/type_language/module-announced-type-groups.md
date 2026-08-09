# Module bodies announce type groups

Retire `RECURSIVE TYPES`; module bodies pre-announce their top-level type
declarations, so any module hosts mutually-recursive nominals.

**Problem.** Mutually-recursive nominal types require the dedicated
`RECURSIVE TYPES <Name> = (body)` block
([`module_def.rs`](../../src/builtins/module_def.rs)): its shallow pre-scan
(`discover_members`) announces member names before their declarations elaborate, which
is what lets cyclic sibling references resolve instead of mutually parking. The block
duplicates module-shaped machinery for that one feature — module bodies already admit
type declarations (mirrored into `Module::type_members`), but a cycle declared in
a `MODULE` body surfaces a chain-gated `unknown type name` forward miss and the
module never binds. The block's body is restricted to `UNION`/`NEWTYPE`
statements — and a `UNION` member never seals at all (the union declarator opens
its own binder window, so the block's announced slot stays unfilled), so the
union half of the surface is designed here, not relocated. The group name binds a
`TypeNode::Group` handle — a type-position binding that admits no values, with
inert predicate arms
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

- *Announcement by shallow pre-scan of top-level statements — decided.* A
  top-level statement announces iff its own parse-time binder plan
  ([`binder.rs`](../../src/machine/model/binder.rs)) matches the `NEWTYPE`/`UNION`
  `= _` specs — full bucket key, never the lead keyword, so a user overload
  sharing a head keyword is excluded; nothing else announces (the
  constructor-family `NEWTYPE (Type AS Wrapper)` key has no Type-part name and
  is skipped, and the `binder_installs` aggregate is not consulted — it answers
  park-edge coverage, not announcement). The top-level boundary is the same one
  the block draws today.
- *Union variants flatten as owned window members — decided (2026-08-07).* A
  top-level `UNION` announces its statically-scannable variant tags as members
  tagged with their owning binder: never bare-name-resolvable, no `bindings.types`
  entry, reachable only through the binder or the qualified sigil. The window's
  single `binder` generalizes to a per-binder map (binder → its variants' union),
  variants join the module window's SCC computation (so union ↔ newtype cycles
  co-seal), and a standalone `UNION` becomes the one-binder special case of the
  same machinery. Canonical component presentation sorts by bare tag with the
  owner as a non-digested tiebreak, so module-hosted variants digest identically
  to their standalone twins and same-tag variants under different binders
  (`:(Graph Node)` / `:(Tree Node)`) coexist: qualified lookup is scoped by the
  binder's member list, identical shapes unify (the structural rule — the binder
  is not in the fold), differing shapes digest apart.
- *Announced names are visible body-wide to consumers — decided (2026-08-07).* A
  plain consumer (an FN signature, a LET ascription) referencing an announced
  member resolves through the window and parks until the window seals — at the
  last announced member's fill, not module close — then re-resolves to the
  absolute handle. Declarator sub-dispatches keep the park-until-*filled* rule
  (park-until-seal would deadlock the group), so resolution carries a
  consumer/declarator mode bit threaded beside the lexical chain.
- *The ambient window is a Drop-free region-bumped record on
  `ScopeKind::Module` — decided (2026-08-07).* Carried as
  `Option<&'a AnnouncedGroup<'a>>`, a `Copy` field before and after seal — the
  `group: Option<&'a OperatorGroup<'a>>` precedent. Possible because announced
  members are always `NewType`-schema'd (fills are `Cell<Option<KType>>`) and
  the member set is fixed at scan. Declarator-local windows (standalone
  declarators, generative `:|` mints) keep a std-owned transient representation
  — they carry `TypeConstructor` schemas and grow by threaded discovery, and
  never ride a scope; the Tarjan/digest seal core is shared as pure functions.
  No `Rc` anywhere.
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
not consumed. Members namespaced inside a module reach bare names again through the
`USING` window, which surfaces a module's type members in type positions
([modules.md § Block-scoped opening](../../design/typing/modules.md)).

**Requires:** none — the substrate the announcement rule needs is shipped.


**Unblocks:**

- [Scopes move into the region bump](../untyped_arena/bump-hosted-scopes.md) —
  retiring the block removes the `Rc`-carrying `RecursiveBlock` scope kind.
