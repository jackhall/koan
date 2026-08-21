# Label interning

Every label the language attaches to a value slot — a record field name, a struct
schema field, an FN parameter name — is syntactic: it originates in source text and
is fixed at declaration. Labels are therefore identified by a run-scoped **symbol**,
a fixed-width content digest, and label *text* lives in one run-scoped side table
consulted only when a label is rendered. No per-record, per-call, or per-node owned
`String` carries a label.

## Terms

- **`Symbol`** — a label's identity: the low 128 bits of a BLAKE3 hash of the
  label's UTF-8 bytes. `Copy`, no lifetime parameter, compared and hashed without
  touching text. `Symbol::of(text)` is a **pure function**: making a symbol needs
  no interner, no registry, no execution context. Equal text yields equal symbols
  in every registry, on every thread, in every run.
- **Label interner** — the digest → text side table (`LabelInterner`), written at
  syntactic-label construction sites and read only by rendering. It is *not* a
  lookup authority: probes and comparisons never consult it.
- **`RunRegistries`** — the run frame's owned bundle of run-lifetime lookup state:
  the [type registry](typing/type-registry.md) and the label interner. A plain
  field on the scheduler-owned run `CallFrame` — no `Rc`, no process-global, no
  `thread_local!` — reached by reference through the execution context and dropped
  with the run frame. The bundle lives on the ordinary heap, not in region
  storage: both registries own growing maps that need `Drop`, and regions are
  Drop-free by design.

## Identity

A symbol is its own lookup key, exactly as a `KType` handle is its digest
([type-identity.md](typing/type-identity.md)). The two share one digest vocabulary:
128-bit truncated BLAKE3, identity-hashed in any map keyed by them, with the same
collision footing — an accidental collision is less likely than a hardware fault,
so symbol equality is label equality with no repair path.

Because a symbol is a pure content function, symbol bits feed type digests
directly: a record type's field names contribute their symbols (fixed-width, no
length prefix) to `TypeDigest` computation, so type identity remains a pure
function of content with no interner in the loop — a hash of hashes, the same
composition the registry already uses for child type digests.

## Residency: owned records only at the intern boundary

`Record<V>` — the ordered, identifier-keyed map behind struct schemas, function
parameter lists, and record values — keys by `Symbol` and is backed by a plain
`Vec<(Symbol, V)>` in insertion order. Lookup is a linear symbol compare (cheaper
than hashing at record sizes), equality is order-blind, and hashing is the
order-blind commutative fold. One heap allocation per record, no index table.

Owned `Record`s exist **only where content outlives every region**: the type
registry's nodes. Everywhere transient, the currency is a borrowed slice
`&[(Symbol, V)]` bumped in whichever region naturally hosts it — a signature's
parameter schema in the definition's region, a call's argument slots on the step
scratch arena, a record literal's field pairs in step construction storage.
Minting a type node copies the slice into an owned `Record` once, at
insert-if-absent time — the single place paying an allocation is amortized,
because equal content interns to one node per run.

The registry's nodes stay lifetime-free: region-bumped slices inside `TypeNode`
would thread a run-region lifetime through `TypeRegistry`, `CallFrame`, and the
ambient context (a self-reference at the harness boundary), which is exactly the
shape the frame system's witnessed-erasure machinery exists to avoid. Registry
content is heap-owned; regions host values.

## Canonical order

Where field order must be canonicalized — digest computation and record-value cell
layout — the canonical order is the **numeric order of the symbols**. Field names
are unique within a record, so symbol order is a total order, deterministic across
registries with no text access.

- `TypeDigest` canonical feeds sort field pairs by symbol before folding.
- A record value's substrate ([value-substrates.md](value-substrates.md)) lays its
  cells out symbol-sorted, indexed by a region-hosted `&[Symbol]` slice; a field
  lookup binary-searches the symbols. No strings live in the substrate.

Insertion (declaration) order is preserved by `Record` itself and remains the
rendering and positional-construction order.

## Argument binding: the schema owns the keys

A call never builds a name-keyed container. The signature builds its parameter
schema **once, at definition**: a region-bumped `&[(Symbol, KType)]` in
declaration order, plus each parameter's slot index into the signature's element
list. This schema is the same record the function's *type* carries — roles shared,
not translated.

Per call, the argument currency is a values-only slice on the step scratch arena,
aligned with the schema: one slot per parameter holding the bound value and its
optional delivery envelope. A named read resolves against the schema
(symbol compare, linear over call arity) and indexes the slot; iteration zips
schema with slots. Dispatch's slot correspondence (`validate_call_args`) is what
makes the positional fill sound. Nothing is re-keyed per call, on either the
builtin or the user-defined lane, and the per-call heap cost of argument passing
is zero.

## Probes never intern

A runtime string probing a record (the `attr` lane) computes `Symbol::of(text)`
and searches the record or substrate directly. A miss is a lookup miss; the
interner is never consulted and never written. Interner growth is therefore
bounded by the run's source text: only syntactic construction sites intern.

## Rendering

Rendering a symbol — printing a record, naming a type, formatting an error —
resolves text through the label interner reached from the execution context via
`RunRegistries`. Pure type-structure questions (subtyping, digests, dispatch)
continue to take the type registry alone; anything that renders text takes the
bundle. A resolve miss renders a stable placeholder rather than failing: error
paths stay total.

## Open work

- [roadmap/reduce_allocs/string-interning.md](../roadmap/reduce_allocs/string-interning.md)
  — the implementation item shipping this design.
