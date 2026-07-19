# USING surfaces module type members

**Problem.** `USING <m> SCOPE <block>` opens a module by overlaying its child scope's
`Bindings` as a transparent scope, and only `data` and `functions` are surfaced —
`Module::type_members` is deliberately excluded
([`using_scope.rs`](../../src/builtins/using_scope.rs)), because the raw child scope
holds the *concrete* type identities, including representations an opaque ascription
(`:|`) hid behind abstract members. The exclusion protects opacity but is blunt: a
module's type members are reachable value-side through `ATTR` (which reads
`type_members` — [`attr.rs`](../../src/builtins/attr.rs)) yet invisible to the sigil
type language, so a type declared in a module cannot be named in type positions from
outside it.

**Acceptance criteria.**

- Inside a `USING m SCOPE` block, the module's type members resolve by bare name in
  type positions — sigil type expressions and dispatch slot types — exactly as the
  module's value members resolve in value positions.
- The surfaced types are read from `Module::type_members` — the module value's own
  view — never from the raw child-scope identities: opening an opaque view surfaces
  its abstract members as `AbstractType` slots, and a test asserts the hidden
  representation type stays unreachable inside the block.
- A bind colliding with a surfaced type member is rejected under the same rule that
  guards surfaced value members.
- The `USING`, module, and ascribe suites are green.

**Directions.**

- *Import channel: the `type_members` mirror — decided.* Reading the module value's
  view preserves opacity by construction (an opaque view's mirror holds `AbstractType`
  slots, not representations) and makes `USING` consistent with `ATTR`, which already
  serves that table.
- *Mechanism — open.* Either pre-install each `type_members` entry into the transparent
  overlay's nominal registrations at block entry, or teach the overlay's type-lookup
  channel to consult the module's mirror; pick whichever keeps `Scope`'s type-lookup
  path single.

## Dependencies

**Requires:** none — `type_members` and the `USING` overlay are shipped surface.

**Unblocks:**

- [Module bodies announce type groups](module-announced-type-groups.md) — retiring
  `RECURSIVE TYPES` namespaces its members inside a module; `USING` is the migration
  path back to bare names.
