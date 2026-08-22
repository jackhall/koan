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

`Record<V>` — the ordered, identifier-keyed map behind struct schemas and function
parameter lists — keys by `Symbol` and is backed by a plain `Vec<(Symbol, V)>` in
insertion order. Lookup is a linear symbol compare (cheaper than hashing at record
sizes), equality is order-blind, and hashing is the order-blind commutative fold.
One heap allocation per record, no index table.

Owned `Record`s exist **only where content outlives every region**: the type
registry's nodes. Everywhere transient, the currency is a borrowed slice
`&[(Symbol, V)]` bumped in whichever region naturally hosts it — a signature's
parameter schema in the definition's region, a call's argument slots on the step
scratch arena, a record literal's field pairs in the destination region's own
construction storage. Minting a type node copies the slice into an owned `Record`
once, at insert-if-absent time — the single place paying an allocation is
amortized, because equal content interns to one node per run.

A record *value* never holds a `Record` at all. The construction doors
(`KObject::record`, `record_of_held`, `record_rehomed`) take the slice currency
directly, and `alloc_record` bumps its two working buffers — the sort buffer and
the cell buffer — in the destination region through the same allocator that hosts
the finished substrate, so building a record of any width takes no heap container.
The one owned `Record` the door mints is the per-field *type* record it hands to
`TypeRegistry::record` to memoize the value's `ktype()` — the intern boundary
again, not a per-value cost.

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
positional-construction order for a type node's field list.

Symbol order carries no meaning to a reader, so a record *value* is rendered
name-sorted: `KObject::summarize` resolves each field's text and sorts on it before
formatting. That is a render-path sort only — the substrate's cell layout stays
symbol-keyed, and the sort is the one place this design adds cost rather than
removing it. It is paid where the alternative is a printed field order that varies
with the hash.

## Argument binding: the schema owns the keys

A call never builds a name-keyed container. The signature builds its parameter
schema **once, at definition**: `params`, a region-bumped `&[(Symbol, KType)]` in
declaration order, beside `part_slots`, each parameter's index into the
signature's element list. This schema is the same record the function's *type*
carries — roles shared, not translated.

Per call, the argument currency is `BoundArgs`: the definition-time schema paired
with a values-only slice on the step scratch arena, aligned slot-for-slot. Each
slot holds the bound cell and its optional delivery envelope. A named read
computes `Symbol::of(name)`, scans the schema (symbol compare, linear over call
arity) and indexes the slot; iteration zips schema with slots. Dispatch's slot
correspondence (`validate_call_args`) is what makes the positional fill sound.
Nothing is re-keyed per call, on either the builtin or the user-defined lane, and
the per-call heap cost of argument passing is zero.

A parameter's schema entry carries its name as a **classified** symbol (below), which
is the same currency the scope binding tables key by. A user-defined call therefore
binds each parameter straight off the schema: no interner reach, and no `String` built
per parameter per call.

## Classified label vocabulary

A binding name's **token class** ([typing/tokens.md](typing/tokens.md)) is carried in the
type of its symbol, not re-derived from text at each door. Three newtypes over `Symbol`
([`labels.rs`](../src/machine/model/labels.rs)) partition the token space:

- **`ValueSymbol`** — a value token (`xs`, `int_ord`, `it`): neither keyword-class nor
  Type-class.
- **`TypeSymbol`** — a Type token per `is_type_name` (`IntOrd`, `Carrier`).
- **`KeywordSymbol`** — a keyword-class token per `is_keyword_token` (`FN`, `+`, `<=`),
  including the space-joined operator probe keys built out of them (`"+ *"`), which gain
  no lowercase letter and so stay keyword-class.

Each is minted **only** by a constructor that runs its class predicate on the text.
There is no raw-`Symbol` constructor: a digest alone carries no evidence of what its text
looked like, so admitting one would let a caller assert a class the digest cannot witness.
A seam holding a bare `Symbol` that needs a class has three doors: it classifies where the
text still exists, it resolves the text through the interner and classifies that, or it
**recovers the class from a classified table**. Recovery is the read-only door: a map keyed
by a classified symbol admits a probe by bare symbol bits (the classified key types
implement `Borrow<Symbol>`), and a hit hands back the *stored* key — a classified symbol
minted where its text existed. Symbol equality is text equality on the shared collision
footing, so the probe's originating text is the key's text and the recovered class is
witnessed, not asserted. Nothing is minted on this path and insertion still requires a
classified key, so a wrong-class probe misses against a map that could never have held it —
the same disposition a wrong-class `of` conversion gets. `WITH` is the canonical user: a
pin arrives as a bare record-field symbol and recovers its `TypeSymbol` from the schema
member table the SIG declaration keyed.

Two constructors, and the split is the interning rule stated above:

- **`of(text)`** — pure classification, no interning. The *probe* constructor: a lookup
  arriving as source text converts once here, and a wrong-class name misses by returning
  `None` at the seam rather than by probing a table.
- **`declared(text, labels)`** — classify **and** intern. The *declaration* constructor:
  a name entering a binding table is recorded so a later diagnostic naming it renders.

**`BinderSymbol`** is the fourth type: an enum over the two *bindable* classes,
`Value(ValueSymbol) | Type(TypeSymbol)`, for a seam that accepts either and routes on the
answer — an FN parameter name, a placeholder install, a module member probe. Its variant
*is* the bind kind, so such a site threads no separate kind tag beside the name. Keywords
are fixed syntax and bind to nothing, so they are not a variant: **nothing binds to a
keyword**, and a keyword-class name mints no `ValueSymbol`. A keyworded dispatch
registration labels a bucket rather than binding a name, so it is untouched by that rule.

## Name-keyed tables

Every name-keyed table keys by this vocabulary, identity-hashed, so a lookup is a
`u128` compare and a key re-homes nothing into a region:

| table | key |
|---|---|
| `Bindings::data` (values) | `ValueSymbol` |
| `Bindings::types` | `TypeSymbol` |
| `Bindings::operators` (probe → group) | `KeywordSymbol` |
| the SIG decl scope's `VAL`-slot collector | `ValueSymbol` |
| a `Module`'s `type_members` / `slot_type_tags` | `TypeSymbol` / `ValueSymbol` |
| a `SigSchema`'s `abstract_members` / `manifest_members` | `TypeSymbol` |
| a `SigSchema`'s `value_slots` | `ValueSymbol` |
| a `NodeSchema::TypeConstructor`'s variant `schema` | `TypeSymbol` |

The claim store's name channel keys by the raw `Symbol`: a claim is stamped before its
producer's kind has settled and spans both bindable classes, and one map stays sound
because the two classes name disjoint text.

Interned [type nodes](typing/type-registry.md) carry the same currency wherever they carry
a binding name: an `AbstractType`'s `name` and `param_names`, a sealed `SetMember`'s
`name`, and a `TypeConstructor` schema's keys and `param_names` are all `TypeSymbol`s. So a
schema's member table and the nodes it holds compare without touching text, cloning a node
copies no name bytes, and the digest feeds those names as fixed-width symbol bits sorted by
those bits — a canonical order over the member set, distinct from the
alphabetical-by-rendered-text order a schema *renders* in. That order is also the canonical
presentation a recursive group's members are indexed in, so a member handle's index and the
digest feed agree by construction.

The currency reaches the value side at the one place a value carries a declared name: a
`KObject::Tagged`'s `tag` is the variant's `TypeSymbol`, so constructing a tagged value
bumps no discriminant bytes into its region and a `MATCH` arm head selects by symbol
compare ([value-substrates.md](value-substrates.md)).

The `data`/`types` partition is therefore a property of the **key types** — a
`ValueSymbol` and a `TypeSymbol` can never wrap the same text, so a name reaching both
maps is unrepresentable rather than something a write door probes for. The partition's
user-visible disposition is raised at the text→symbol declaration seam
([typing/elaboration.md § Binding-map partition](typing/elaboration.md#binding-map-partition)),
which is the one place the rule is a runtime answer rather than a type.

Conversion sits at the **lookup seam**, not the parse boundary: a resolve ladder takes
`&str`, classifies once at the top with `of`, and compares symbol bits from there down.
A wrong-class probe misses at that conversion — against a map that could never have held
such a key.

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
