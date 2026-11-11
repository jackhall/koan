# A nominal over a run-bound type, typed at load

A `NEWTYPE` or `UNION` whose definition names a type a run binds, given one
load-time handle rather than declared at every call.

**Problem.** A type binder is closed or unknown at load, never rigid
([what stays unknown](../../src/elaborate/README.md#the-type-channel-at-load)):
a sealed nominal is a leaf substitution never enters, so the declaration door
reads every run-bound name as unknown. Under

```koan
EXPR FOR ALL #[Elt] #(BOXIT x :Elt) -> Any = #(
  NEWTYPE Box = :{v :Elt}
  (Box {v = x})
)
```

`Box` is left to the layer whose binding closes `Elt`, every reader of `Box`
in the body is unknown with it, and the declaration runs through the
activation each time its unit runs. A nominal's identity is its name and
schema, so two calls at one binding of `Elt` give one type —
`(BOXIT 1) == (BOXIT 1)` is `true` — and calls at different bindings give
different types; what the program pays for is a binder the load cannot type
and a declaration repeated per call.

**Acceptance criteria.**

- `Box` above is rigid at load: one load-time handle over `Elt`, and each of
  its readers in the body is a rigid fact, not an unknown one.
- At a run, `Box` is that handle substituted at the call's binding, the same
  type the activation's declaration gives: `(BOXIT 1) == (BOXIT 1)` is `true`,
  and a `Box` built at `Number` and one built at `Str` are different types.
- A nominal in a recursive group over a run-bound type — a `Box` whose schema
  names `Box` or a sibling — is rigid at load the same way.
- A law in the [property suite](../../lattice/src/types/tests/properties.rs)
  states that substituting into a sealed group and re-sealing gives the group
  sealed over the substituted schemas.

**Directions.**

- *How the lattice substitutes into a sealed group — open.* A door that
  substitutes into the sealed group's schemas and re-seals it, the result's
  identity its name and substituted schema as a declaration's is; or the
  nominal lifted into a family over the run-bound names it reads, applied at
  their binding as a parameterized `NEWTYPE` is. Either is a change to the type
  lattice, a flagged exception needing an OK.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
