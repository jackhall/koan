# Run-scoped label interning

**Problem.** Every place the language labels something pays an owned `String`, even though
the label text is syntactic and already lives somewhere stable. Passing arguments into a
call clones a `String` per parameter, per call: `map_arg_carriers`
([src/machine/execute/decide/exec.rs](../../src/machine/execute/decide/exec.rs)) builds a
`Record<&DeliveredCarried>` by `record.insert(arg.name.to_string(), carrier)`, where
`arg.name` is a region-bumped `&'a str` that outlives the call — and the builtin lane pays
the pair twice, once for the carriers and again for the `Record<Held>` that `bind_args`
returns. `Record<V>` ([src/machine/model/types/record.rs](../../src/machine/model/types/record.rs))
is an `IndexMap<String, V>` used in ~74 places, so every record value, record type node,
and signature schema owns a fresh copy of each field name — plus `IndexMap`'s second
(index-table) allocation per record. Every construction site keys off a syntactic name —
signature parameter names, field-list labels, type-node fields, fixed literals — never a
runtime-computed string; runtime strings appear only in lookup (`read_field_name` in
[src/builtins/attr.rs](../../src/builtins/attr.rs) probes an existing record), which never
inserts a key. The scope tables in
[src/machine/core/bindings.rs](../../src/machine/core/bindings.rs) already key by
region-bumped `&'a str` and are not part of this problem.

The design this item ships is pinned in
[design/label-interning.md](../../design/label-interning.md); the implementation route is
`scratch/string-interning-plan.md`.

**Acceptance criteria.**

- A `RunRegistries` bundle — the type registry beside a label interner — is a plain owned
  field on the scheduler-owned run frame (no `Rc`, no bump-hosting), reached by reference
  through the execution context; there is no process-global state, and both registries
  drop with the run frame.
- A label handle (`Symbol`) is a fixed-width `Copy` type with no lifetime parameter,
  compared and hashed without touching the text; `Symbol::of(text)` is a pure function
  needing no interner reach.
- `Record<V>` keys by `Symbol` over a `Vec<(Symbol, V)>` backing (no `IndexMap`), with
  linear lookup and the order-blind equality/hash invariants preserved; owned `Record`s
  appear only in type-registry-resident content, and every transient record travels as a
  region- or scratch-bumped `&[(Symbol, V)]` slice.
- A signature owns its parameter schema from definition time: a region-bumped
  `&[(Symbol, KType)]` in declaration order, shared with the function type's parameter
  record; a call carries only a values slice on the step scratch aligned to that schema,
  so no name-keyed container is built per call on either lane and `map_arg_carriers` no
  longer exists.
- Record field names contribute their symbol bits (fixed-width) to `TypeDigest`
  computation, and every canonical field ordering — digest feeds and record-value
  substrate cell layout — sorts by symbol numeric order; the substrate's field index is a
  region-hosted `&[Symbol]` with no strings.
- A runtime string value used to probe a record (the `attr` lane) resolves by computing
  `Symbol::of(text)` and searching, never touching the interner: a miss is a lookup miss,
  and interner growth is bounded by the run's source text.
- Rendering a `Symbol` (printing, error messages) resolves the text through the interner
  reached from the execution context, and a resolve miss renders a stable placeholder
  rather than panicking.
- The recorded per-call allocation count for a builtin call with n parameters drops by
  the 2n names plus both argument containers.

**Directions.**

- *Handle identity — decided.* A content digest of the text, identity-hashed, following
  [`TypeRegistry`](../../src/machine/model/types/registry.rs)'s digest-keyed design: the
  handle is its own lookup key, equal text yields equal handles across registries, and
  the structure-sharing cross-thread transfer story carries over. Sequential indexes are
  rejected — they are registry-relative, and they put the interner on the construction
  and digest paths.
- *Digest width — decided.* The full 128-bit truncated-BLAKE3 shape: one digest
  vocabulary across both registries, the same collision-is-a-hardware-fault footing as
  type identity, and symbol bits feed type digests in the established width.
- *Argument currency — decided.* The schema-keyed view per
  [design/label-interning.md](../../design/label-interning.md): the signature's
  definition-time parameter record owns the keys; a call pays a values-only scratch
  slice, zero heap containers. Both per-call `Record` currencies and the free `arg_*`
  helper family are deleted.
- *Registry hosting — decided.* Owned `RunRegistries` field on the run frame — not
  `Rc`-shared (nothing needs shared ownership) and not region-bumped (the registries own
  heap maps that must `Drop`; regions are Drop-free).

## Dependencies

**Requires:** none.

**Unblocks:** none.
