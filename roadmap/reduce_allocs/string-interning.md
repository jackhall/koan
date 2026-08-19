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
and signature schema owns a fresh copy of each field name. Every construction site keys
off a syntactic name — signature parameter names, field-list labels, type-node fields,
fixed literals — never a runtime-computed string; runtime strings appear only in lookup
(`read_field_name` in [src/builtins/attr.rs](../../src/builtins/attr.rs) probes an
existing record), which never inserts a key. The scope tables in
[src/machine/core/bindings.rs](../../src/machine/core/bindings.rs) already key by
region-bumped `&'a str` and are not part of this problem.

**Acceptance criteria.**

- A run-scoped string interner hangs off the scheduler-owned run frame beside
  `CallFrame::type_registry`, reached by reference through the execution context; there is
  no process-global state.
- A label handle (`Symbol`) is a fixed-width `Copy` type with no lifetime parameter,
  compared and hashed without touching the text.
- Re-keying slot-indexed argument carriers onto their parameter names allocates no
  `String`: the parameter name crosses as a `Symbol`, and the user-defined-call lane's
  argument values reach the frame bind through the same currency, so a call pays one
  container at most, not two.
- `Record<V>` keys by `Symbol`; record construction interns each syntactic label once per
  run rather than allocating an owned key per record.
- A runtime string value used to probe a record (the `attr` lane) resolves against the
  interner without inserting: a miss is a lookup miss, never a new interned entry, so
  interner growth is bounded by the run's source text.
- Rendering a `Symbol` (printing, error messages) resolves the text through the interner
  reached from the execution context.
- The recorded per-call allocation count for a builtin call with n parameters drops by
  the 2n names plus the second container.

**Directions.**

- *Handle identity — decided.* A content digest of the text, identity-hashed, following
  [`TypeRegistry`](../../src/machine/model/types/registry.rs)'s digest-keyed design: the
  handle is its own lookup key, equal text yields equal handles across registries, and
  the structure-sharing cross-thread transfer story carries over. Sequential indexes are
  rejected — they are registry-relative.
- *Digest width — open.* The low 64 bits of the BLAKE3 hash versus the full 128-bit
  `TypeDigest` shape. 64 halves the handle but gives up the collision margin the type
  registry relies on; 128 keeps one digest vocabulary across both registries.
- *Argument-currency container — open.* With `Symbol` keys, the transient argument
  currency can stay `Record`-shaped (now allocation-free per key) or become a
  `(Symbol, V)` slice sized off the signature. Recommended: whichever leaves `Record<V>`'s
  API a single keying story; the slice is only worth it if the map's own allocation still
  shows up after the keys are free.

## Dependencies

**Requires:** none.

**Unblocks:** none.
