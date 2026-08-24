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
  lookup authority: probes and comparisons never consult it. One table serves a
  whole run: it is created **before parse**, handed to the parser, and adopted by
  the run frame, so the entries the parse boundary wrote are the ones a diagnostic
  later resolves through.
- **`RunRegistries`** — the run frame's owned bundle of run-lifetime lookup state:
  the [type registry](typing/type-registry.md) and the label interner. A plain
  field on the scheduler-owned run `CallFrame` — no `Rc`, no process-global, no
  `thread_local!` — reached by reference through the execution context and dropped
  with the run frame. It is minted by `RunRegistries::with_labels` when the run
  frame is established, adopting the interner the parse filled rather than opening
  an empty one beside it
  ([interpret.rs](../src/machine/execute/interpret.rs) is the ladder that threads
  it). The bundle lives on the ordinary heap, not in region
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
slot holds the bound cell and its optional delivery envelope. A named read takes
the slot's declared name (below), reads the symbol off its memo, scans the schema
(symbol compare, linear over call arity) and indexes the slot; iteration zips
schema with slots. Nothing on that path hashes. Dispatch's slot
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

Two text constructors, and the split is the interning rule stated above:

- **`declared(text, labels)`** — classify **and** intern. The *declaration* constructor:
  a name entering a binding table is recorded so a later diagnostic naming it renders. Its text
  is chosen at runtime — a name the program spelled, or one the machine picks out of a value in
  hand. Classification and recording share the digest classification already took, so a
  declaration hashes its text once. A name the machine fixes in its own source takes the third
  door instead, below, which reaches the interner without classifying anything.
- **`of(text)`** — pure classification, no interning. The *probe* constructor: a lookup
  arriving as source text converts once here, and a wrong-class name misses by returning
  `None` at the seam rather than by probing a table. **Only `KeywordSymbol` exposes it** — the
  operator and dispatch tables key by fixed tokens read back out of source, so the keyword side
  is the one vocabulary a bare spelling still legitimately enters. Neither *name* class has a
  peer: a name is minted at the parse that classifies it, on both sides of the partition, and
  every later reader carries that symbol rather than re-classifying a spelling. A seam that must
  probe a name-keyed table from runtime text derives a bare `Symbol` and compares bits, or takes
  the `declared` door above when it wants the class as well.

Both run the class predicate through one hidden `classify` funnel, shared with the
`static_name!` mint below, so a class has exactly one implementation of "does this text
classify".

**`BinderSymbol`** is the fourth type: an enum over the two *bindable* classes,
`Value(ValueSymbol) | Type(TypeSymbol)`, for a seam that accepts either and routes on the
answer — an FN parameter name, a placeholder install, a module member probe. Its variant
*is* the bind kind, so such a site threads no separate kind tag beside the name. Keywords
are fixed syntax and bind to nothing, so they are not a variant: **nothing binds to a
keyword**, and a keyword-class name mints no `ValueSymbol`. A keyworded dispatch
registration labels a bucket rather than binding a name, so it is untouched by that rule.

## Names fixed in Rust source

Some labels are not read out of a program at all: a builtin's parameter slot, the `Result`
family's `Ok` / `Error` tags, the `KError` family name, the `it` an arm binds its scrutinee
under. Their spelling is `&'static str` written
in Rust, so their symbol is the same 128 bits for the whole process and there is nothing for a
per-run or per-call classification to discover. Such a name is **declared once and compared by
symbol thereafter**.

The declaration is a `StaticName<S>` ([`labels.rs`](../src/machine/model/labels.rs)): the
spelling beside a `LazyLock` memo of its classified symbol, built by the `static_name!` macro,
which mints through the class's own `classify` funnel. A `LazyLock` over a pure function of a literal
is a memo and not run state — `Symbol::of` answers the same bits in every run and every process,
so the cached value carries nothing from one run into the next. The class predicate still runs on
the same text it would have run on, once, at first touch; a spelling that will not classify
panics there naming itself and the class it failed.

A builtin's parameter slots declare through `slots!` instead, which takes the whole group as
idents — `slots! { SLOTS { left, right } }`, read as `&SLOTS.left` — and `stringify!`s each ident
into the literal its mint hashes. The spelling is written once, so the one a signature registers
and the one its body reads back cannot be spelled differently. Value class is the entire
vocabulary of a slot, so the group is `StaticName<ValueSymbol>` throughout and names no class of
its own; `static_name!` stays the door for a name that is not a slot, where the class has to be
said. Grouping is placement and nothing else — each field is its own `StaticName`, forced
independently at its first read, so a group of *n* slots mints the same *n* symbols the same
slots declared one at a time would.

`S` ranges over the **`ClassifiedSymbol`** types — a sealed trait exposing the raw digest, so a
generic seam can compare and intern without knowing the class. Its implementors are the three
class newtypes plus `BinderSymbol`, which is the classified vocabulary entire; sealed, because a
further implementor would be a class the token grammar does not have.

`LabelInterner::record(&StaticName<S>) -> S` is the declaration door for such a name: it reads
the symbol off the memo and records the spelling under it, so a run gets its interner entry — a
diagnostic naming the slot still renders — at the cost of one map lookup and no hash. That is the
whole difference from `declared`, which has to classify text it is seeing for the first time.

**Where a name is declared is where it is used.** Each builtin file declares the spellings its own
bodies and registrations name as one group, beside them; a child module borrows its parent's
through `super::`. There is no central slot module, and a group is not a registry: a shared table
could only deduplicate mint *count*, never identity. Two builtins that spell a slot `name` already share one
symbol, because `Symbol::of` is a pure function of text and does not know which declaration minted
first. What the per-file placement does buy is that a slot's spelling sits next to the body it
parameterizes.

The consumers are the two ends of one slot:

- Registration. [`builtins::arg`](../src/builtins.rs) takes a `&StaticName<ValueSymbol>` and
  records it. A slot is therefore value-class **by declaration** — the class is settled where the
  spelling is written rather than probed at registration.
- Body reads. `BoundArgs`'s named doors (`held` / `object` / `ktype` / `unresolved_type` /
  `carrier`) and the `require_*` helpers take the same `&StaticName<ValueSymbol>`
  ([action.rs](../src/machine/core/kfunction/action.rs)), so the static a signature registers and
  the static a body reads are one item and cannot drift apart. A diagnostic that has to name the
  slot renders its `text()`, which is the spelling as written.

The Rust-side tags are the same shape one level up: `Ok`, `Error` and `Result` are declared in
[result.rs](../src/builtins/result.rs) and `KError` in
[kerror.rs](../src/machine/core/kerror.rs), each a `StaticName<TypeSymbol>` recorded at the
registration that builds its type. `CATCH` reads the tags back through `result`'s own statics, so
the tag a `Result` value is built under is the tag its registration declared. A name chosen at
runtime takes `declared`: a `KError` variant's name is picked by the variant in hand rather than
fixed at a source site. The builtin type vocabulary is fixed in source like the tags —
[builtin_names.rs](../src/machine/model/types/builtin_names.rs) declares the eleven spellings as
`StaticName<TypeSymbol>`s beside the handle each lowers to, and that one table is what both root
registration and `KType::from_symbol` read, so a builtin type name is matched by eleven symbol
compares and never classified from text.

A name the machine *binds into a program's scope* is fixed in source the same way. `it` — the
scrutinee binder every `MATCH` and `TRY` arm opens — is a `StaticName<ValueSymbol>` declared in
[branch_walk.rs](../src/builtins/branch_walk.rs) beside the arm tail that binds it, and read
through `record` there. It is value-class like a parameter slot but is not one: no signature
registers it, and the arm binds under the symbol the static already holds, so an arm taken hashes
no text.

The mint count is measured, not bounded, in
[audit/README.md § Symbol mints](../audit/README.md#symbol-mints).

## Name-keyed tables

Every name-keyed table keys by this vocabulary, identity-hashed, so a lookup is a
`u128` compare and a key re-homes nothing into a region:

| table | key |
|---|---|
| `Bindings::data` (values) | `ValueSymbol` |
| `Bindings::types` | `TypeSymbol` |
| `Bindings::operators` (probe → group) | `KeywordSymbol` |
| `Bindings::functions` and the claim store's bucket channel | `&[KeyElement]`, a run of `Keyword(KeywordSymbol)` / `Slot` |
| the SIG decl scope's `VAL`-slot collector | `ValueSymbol` |
| a `Module`'s `type_members` / `slot_type_tags` | `TypeSymbol` / `ValueSymbol` |
| a `SigSchema`'s `abstract_members` / `manifest_members` | `TypeSymbol` |
| a `SigSchema`'s `value_slots` | `ValueSymbol` |
| a `NodeSchema::TypeConstructor`'s variant `schema` | `TypeSymbol` |

The claim store's name channel keys by the raw `Symbol`: a claim is stamped before its
producer's kind has settled and spans both bindable classes, and one map stays sound
because the two classes name disjoint text.

A dispatch bucket key is the one composite in that table: not a single label but a run of
positions, a keyword's `KeywordSymbol` where the shape fixes a token and `Slot` where it
takes an argument ([`KeyElement`](../src/machine/model/types/signature.rs)). The element is
`Copy` and lifetime-free, so the run a caller owns and the run a scope bumped into its region
are the same type — one derived `Hash`, and a key re-homes by copying `u128`s rather than
keyword bytes. Rendering such a key names each keyword by resolving its symbol, on the same
miss-renders-a-placeholder rule as any other label.

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

## Where text becomes a symbol

The conversion seam is per vocabulary, and the answers differ because the vocabularies are
reached differently: two convert where the parser classifies the token, one where a lookup
consults a table.

The **keyword** vocabulary converts at the **parse boundary**. Where the parser classifies a
token as keyword-class ([tokens.rs](../src/parse/tokens.rs)) it mints the token's
`KeywordSymbol` and interns it in the same step, and the part carries a `KeywordToken` —
program-storage text beside that symbol ([ast.rs](../src/machine/model/ast.rs)) — from then
on. Nothing downstream re-hashes: a node's bucket key, a signature element, an operator
chain's cached registry probe and every keyword comparison read the symbol the parse already
minted. Registration re-interns at `ExpressionSignature::mint`, because a draft may spell a
token lowercase and the bucket it keys is the normalized spelling. That placement is not
convenience — `Symbol::of` is a BLAKE3 hash, and a keyword sits on the hot dispatch probe
path, where paying one per keyword per call is exactly the cost a parse-time cache exists to
remove.

The **type** vocabulary converts there too, on the same reasoning. Where the parser classifies a
token as Type-class it mints the token's `TypeSymbol` through `declared` and the part carries the
symbol alone — `ExpressionPart::Type(TypeSymbol)`, no surface text beside it. Every later reader
carries that symbol through: the bind seam's `UnresolvedType` carrier, the type-side lookup ladder
(`Scope::resolve_type_with_chain`), `elaborate_type_identifier`, a variant-tag match, and each
type-declaring builtin's binder name. Because the symbol is lifetime-free and `Copy`, a type name
crossing a region boundary is copied rather than re-bumped, and a name that must be *printed*
resolves back through the interner.

The **value** vocabulary converts at the parse boundary too, and for the same reasons — the two
name classes are mirror images. Where the parser classifies a token as neither keyword-class nor
Type-class it mints the token's `ValueSymbol` through `declared`, and the part carries the symbol
alone (`ExpressionPart::Identifier(ValueSymbol)`, `FieldSlot::Name(ValueSymbol)`). Every later
reader carries it: the bind seam's `Held::Identifier` carrier — the value-channel mirror of
`Held::UnresolvedType`, read back through `BoundArgs::identifier` and nothing else — the
value-lookup ladder (`Scope::resolve_value_delivered`, which takes a `ValueSymbol` rather than a
spelling), each binder builtin's own name read, and the statement's `StoredBinderKey`, whose
`to_owned_key` mints nothing. Because no lookup re-derives a digest, a name spelled once and read
many times is hashed once, at the parse.

The consequence for the surface is that neither name class admits a bare probe. A binder builtin
that needs the spelling back — for a diagnostic that quotes the name — renders it out of the run's
interner rather than holding text beside the symbol.

## Probes never intern

A runtime string probing a record computes `Symbol::of(text)` and searches the
record or substrate directly. A miss is a lookup miss; the interner is never
consulted and never written.

The **derived-symbol door** is the exception, and there is exactly one on each
side of the name partition. On the value side it is
[`classify_derived_field`](../src/builtins/attr.rs), which every member name that
reaches `ATTR` as text rather than as a parse-minted token funnels through — a
rendered type handle, or the runtime string the dynamic `ATTR <s> <field :Str>`
overloads read. It classifies through `BinderSymbol::declared`, so a spelling read
off text keys the same symbol a bare token of that spelling would have minted, and
`s."x"` and `s.x` reach one member. Classifying through `declared` means it also
**interns**: a computed member name is recorded, so interner growth is bounded by
the run's source text *plus* the distinct member names a run computes. That is the
price of the two spellings agreeing, and it is paid only by a program that names a
member at runtime. Text classifying as neither bindable class names no binding at
all, so it never reaches the interner — it rides as a rendering, which is a
digest-keyed record probe and an immediate module miss.

## Rendering

Rendering a symbol — printing a record, naming a type, formatting an error —
resolves text through the label interner reached from the execution context via
`RunRegistries`. Pure type-structure questions (subtyping, digests, dispatch)
continue to take the type registry alone; anything that renders text takes the
bundle. A resolve miss renders a stable placeholder rather than failing: error
paths stay total — that is `LabelInterner::render`, the total form every render
path uses. Its `display` twin is the same read as a `Display` view, so a message
that names a label writes the recorded text straight into the message's own
buffer with no `String` in between; a diagnostic built on a path that succeeds
costs no more than one built from a borrowed name.

Because the surface parts of an expression carry symbols rather than text,
`summarize` — the surface rendering of a part, an expression or a trace frame —
takes the run's interner, and so do the parse-side walkers that render a name
back out of a part (`parse_pair_list`, the record-literal field-name read).

## Open work

- [roadmap/reduce_allocs/symbol-keyed-field-lists.md](../roadmap/reduce_allocs/symbol-keyed-field-lists.md)
  — record-literal keys, field lists and FN parameter names as symbols.
- [roadmap/reduce_allocs/symbol-only-keyword-tokens.md](../roadmap/reduce_allocs/symbol-only-keyword-tokens.md)
  — keyword parts drop their carried spelling.
