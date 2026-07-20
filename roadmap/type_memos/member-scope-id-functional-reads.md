# Retire functional `NominalMember.scope_id` reads

Three sites still key control flow on a set member's `scope_id`. Member `scope_id` is
digest-excluded content, so under interned type nodes it is first-interner-wins and
cannot carry a decision.

**Problem.** A [`NominalMember`](../../src/machine/model/types/recursive_set.rs) records
the `ScopeId` that declared it, and that field is excluded from the set digest — two
digest-equal sets may carry different member `scope_id`s, and interning would collapse
them to one node whose `scope_id` is whichever declaration interned first
([type-identity.md](../../design/typing/type-identity.md)). Three sites nonetheless read
it functionally rather than for diagnostics:

- `KTypeUserRefs` in
  [`resolve_type_identifier.rs`](../../src/machine/execute/dispatch/resolve_type_identifier.rs)
  yields `(member.scope_id, member.name)` for every `SetRef`, and `FinalizeGate` walks
  ancestors for a scope with that id to decide whether the memo must park on an
  in-flight producer. The scope the reference actually resolved through is the datum it
  wants; the placeholder's pre-installed set is already reachable, so an `Rc::ptr_eq`
  against it decides the same question without the id.
- `finalize_nominal_member` in
  [`resolver.rs`](../../src/machine/model/types/resolver.rs) compares
  `member.scope_id == scope_id` to tell a parallel finalize of *this* declaration
  (idempotent short-circuit) from a genuine prior binding of the same name (a `Rebind`).
- `recover_union` in [`union.rs`](../../src/builtins/union.rs) compares
  `member0.scope_id == scope_id` for the same same-declaration question on the union
  path.

The latter two ask "is this the binding my own declaration installed?" — a question the
binding index answers directly, since `lookup_type` already knows which binding it
returned.

**Acceptance criteria.**

- No site outside diagnostics reads `NominalMember.scope_id`: `KTypeUserRefs` no longer
  yields it, and the two same-declaration checks decide on binding identity threaded
  through `lookup_type` rather than on a recorded scope id.
- `FinalizeGate::pending_producers` parks on exactly the producers it parks on today,
  pinned by the existing forward-reference suite
  ([`forward_reference_resolves.rs`](../../tests/forward_reference_resolves.rs)).
- Redeclaring a nominal or a union in the same scope still raises `Rebind`, and a
  parallel finalize of one declaration still short-circuits idempotently.
- `NominalMember.scope_id` is either deleted or documented as diagnostics-only, so an
  interned node's first-interner-wins value cannot change behavior.

**Directions.**

- *`FinalizeGate` decides by set identity — decided.* `Rc::ptr_eq` against the
  placeholder's pre-installed set answers "is this reference to a still-unsealed
  declaration in scope" without naming a scope.
- *Same-declaration checks key on the binding index — decided.* `lookup_type` already
  resolves to a binding; threading its `BindingIndex` out lets both callers compare
  identities instead of reconstructing one from a scope id.
- *Delete the field or keep it for diagnostics — open.* Deleting it is the strongest
  guarantee; keeping it costs a `ScopeId` per member and preserves error messages that
  name the declaring scope. Recommended: keep it, documented as diagnostics-only, and
  revisit when the interned-node work lands.

## Dependencies

**Requires:** none — the three read sites are shipped surface.

**Unblocks:**

- [Interned type content behind Copy handles](interned-type-content.md) — interning
  makes a member's digest-excluded `scope_id` first-interner-wins, so no decision may
  rest on it.
