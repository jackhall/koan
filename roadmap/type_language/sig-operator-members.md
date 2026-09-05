# SIG operator members

A signature can declare an operator the way it declares any other keyworded member, and an
ascription view carries enough for `USING <view> SCOPE` to reduce runs by it.

**Problem.** An `OP` declaration writes two channels: dispatch-bucket overloads (a binary
operator under its `[Slot, <sym>, Slot]` key, a unary operator's list-form and bridge pair) and
one entry in the per-scope operator registry — the chaining mode the run reducer walks
([design/operators.md](../../design/operators.md)). The bucket half already reaches the
keyworded surface: `SigSchema::raw_self_sig`
([sig_schema.rs](../../src/machine/model/types/sig_schema.rs)) projects every bucket the body
registered, so a module declaring `OP #(+) OVER Number` carries that overload in its self-sig.
But a SIG cannot *declare* the member: the bodyless keyworded declarator parses an FN-style
element sequence, and an unquoted operator symbol there is claimed by the chain reducer rather
than landing as a key element — the exact problem the `#(…)` quote solves for `OP` itself. And
the registry half is not schema content at all: satisfaction never examines a chaining mode, and
an ascription view's operator registry is empty, so even where a source module's operator
overload would satisfy a declared keyworded member, a run inside `USING <view> SCOPE` cannot
reduce by it.

**Acceptance criteria.**

- A SIG body declares an operator member with a bodyless `OP` head carrying the quoted symbol —
  `(OP #(+) OVER Carrier)` — and the member projects into the schema's keyworded channel under
  the same key the definition registers.
- Satisfaction admits a module for a declared operator member through the same most-specific
  selection every keyworded member takes, and the three keyworded failure diagnostics name the
  operator head.
- An ascription view installs both halves of a declared operator: the selected overload
  (coerced across an opaque barrier exactly as any keyworded member is) and the operator-registry
  entry, so a run inside `USING <view> SCOPE` reduces by the declared operator.
- Two modules whose operators differ only in chaining mode are distinguished — or deliberately
  not distinguished — by satisfaction per a settled rule, pinned by a test and documented in
  [design/typing/modules.md](../../design/typing/modules.md) and
  [design/operators.md](../../design/operators.md).
- The pairwise heterogeneous form (`OP #(<) OVER Number -> Bool`) and the unary form are each
  either declarable or refused at the declaration with a pointed diagnostic — settled, not
  silently absent.

**Directions.**

- *Declarator spelling — open.* A bodyless `OP` head mirroring the definition form (as the FN
  head mirrors the FN definition) keeps declaration and definition deriving key and slot types
  from one implementation; the alternative — teaching the FN-head declarator to accept a quoted
  symbol element — spreads the operator surface across two lead keywords. Recommended: the
  bodyless `OP` head.
- *Chaining mode as signature content — open.* Either the mode rides the schema (it feeds the
  content digest, satisfaction requires agreement, and the view install reads it from the
  member), or it stays module-local and the view copies the source's registry entry unchecked.
  The mode changes what a run of the operator means at the use site, which argues for schema
  content.
- *The unary triple — open.* A unary definition installs a triple (list-form overload, binary
  bridge, size-1 registry entry). Whether a unary declarator names the triple as one member or
  the list-form overload alone needs settling before the view install can be specified.

## Dependencies

**Requires:** none — extends the shipped keyworded surface.

**Unblocks:**

- [Expression shapes are their own kind of function](expression-shapes.md) — the shape
  representation must cover operator members, so their surface is settled first.
