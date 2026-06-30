# Enforce the type/value split in Bindings

Make "this name is a type" versus "this name is a value" a structural property of the
`Bindings` API, not an invariant maintained by convention at each bind site.

**Problem.** [`Bindings`](../../src/machine/core/bindings.rs) already stores committed
type bindings and value bindings in separate maps — `types` keyed to `&KType` and
`data` keyed to `&KObject` — and the LET boundary enforces the partition (a type binds
only under a Type-classified name; `LET t = Point` under a value-classified name is
rejected). But the split is held by per-callsite discipline, not by the API:

- The in-flight `placeholders` map keys a bare name to a `(NodeId, BindingIndex)` with
  no type/value discriminant, so a forward reference's kind is unknown until it
  resolves and a value placeholder is indistinguishable from a type placeholder under
  the same name.
- Read sites consult both maps by hand:
  [`access_module_member`](../../src/builtins/attr.rs) probes `data` then `types` in
  sequence, with nothing guaranteeing a name resolved in exactly one.
- There is no single typed operation that makes "bind a value" and "bind a type"
  mutually exclusive by construction; the invariant lives at each declarator (LET, VAL,
  FN, FUNCTOR, MODULE, SIG, NEWTYPE, UNION, RECURSIVE TYPES).

So the committed storage is partitioned, but the partition is a property the call sites
agree to uphold rather than one the type makes unbreakable.

**Acceptance criteria.**

- Whether a name is a type binding or a value binding is structural: one classified
  entry point binds each kind, and it is impossible — not merely disallowed by
  convention — for one name to hold both, or for a lookup to return the wrong kind.
- The in-flight placeholder/pending machinery carries the type/value axis, so a forward
  reference's kind is known before it resolves and a type placeholder is never
  satisfied by a value bind (or vice versa).
- ATTR module/signature member access obtains a value-or-type through one classified
  lookup rather than probing the value map then the type map.
- A test asserts a value-classified and a type-classified bind of the same name cannot
  coexist or resolve ambiguously across every bind site.

**Directions.**

- *Where the discriminant lives — open.* Either (a) merge the two committed maps into
  one `HashMap<String, Binding>` whose value is a `{ Value | Type }` sum (the map
  enforces one entry per name), or (b) keep the separate maps but route every
  bind/lookup through a classified API that rejects cross-kind collisions. Recommended:
  (b) — it keeps the fast kind-specific lookups while moving the invariant into the API.
- *Placeholder kind — decided.* The `placeholders` map gains a value/type discriminant
  so a forward reference resolves only against a same-kind bind.
- *Classification rule unchanged — decided.* Name classification stays lexical (Type
  tokens versus identifiers, `is_type_name`); this item enforces the storage/lookup
  partition, not the classification rule.

## Dependencies

An engine-internal binding-layer hygiene item, adjacent to
[Unify the value-name lookup outcomes](unify-name-lookup-outcome.md) (both touch
`Bindings` and its lookup outcomes). Update
[design/typing/lookup-protocol.md](../../design/typing/lookup-protocol.md) if the
binding/partition vocabulary it names changes.

**Requires:** none — engine-internal.

**Unblocks:**
- [Structural witnesses](../per-node-memory/structural-witnesses.md) — the classified binding entry
  this lands is where structural-witnesses hangs each value binding's retained carrier.
