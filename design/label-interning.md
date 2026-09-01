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

`Record<V>` — the ordered, name-keyed map behind struct schemas and function
parameter lists — keys by [`BinderSymbol`](#classified-label-vocabulary) and is backed by a
plain `Vec<(BinderSymbol, V)>` in insertion order, so a field name carries the class its own
declaration classified it into rather than dropping it at the intern boundary. Identity is
the key's `Symbol` bits alone: equality, hashing and the type digest read `key.symbol()` and
never the variant tag, so carrying the class widens nothing about what makes two records the
same. Lookup is a linear symbol compare (cheaper than hashing at record sizes), equality is
order-blind, and hashing is the order-blind commutative fold. One heap allocation per record,
no index table.

Owned `Record`s exist **only where content outlives every region**: the type
registry's nodes. Everywhere transient, the currency is a borrowed slice
`&[(BinderSymbol, V)]` bumped in whichever region naturally hosts it — a signature's
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

The two halves of that door are where the currency **splits**. A value's cells are laid out
against a bare `&[Symbol]` index — a cell lookup is a name probe, and a probe carries no class
— while the memoized type record keeps the classified key the declaration or the literal's own
token minted. So `KObject::record_of_held` takes classified pairs, keys the type record with
them, and drops to `key.symbol()` for the substrate in one line; `record_rehomed`, which
relocates cells already laid out, stays bare-`Symbol` throughout.

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
name-sorted: the render arm stages the field symbols and orders them through
`LabelInterner::compare_texts`, which compares the recorded slices in place, so the
sort reads text without rendering any of it. That is a render-path sort only — the substrate's cell layout stays
symbol-keyed, and the sort is the one place this design adds cost rather than
removing it. It is paid where the alternative is a printed field order that varies
with the hash.

## Argument binding: the schema owns the keys

A call never builds a name-keyed container. The signature builds its parameter
schema **once, at definition**: `params`, a region-bumped `&[(BinderSymbol, KType)]` in
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
  including the operator probe keys `KeywordSymbol::of_run` digests out of a run of them:
  a run of keyword-class tokens names fixed syntax and binds to nothing, which is what the
  class stands for.

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
member table the SIG declaration keyed. A field record is the other: `Record::get_key_value`
answers a bare-symbol probe with the stored `BinderSymbol`, which is how `FROM` narrows a
record type to the fields it names while keeping each key's declared class.

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

A **caught error's field names** are fixed in source too, and they are the case where the group is
value-class without being slots: `arg`, `expected`, `got`, `message`, `frames` and the rest of the
error record's labels are `StaticName<ValueSymbol>`s held in one `ErrorFields` static beside the
`KErrorKind` match that builds each shape ([kerror.rs](../src/machine/core/kerror.rs)). A handler
reads `message` or `expr` off a caught error by name, so the label is an ordinary record key — but
the spelling is the machine's, not a program's, so it classifies at its first read like any other
Rust-source name and `record` hands the lowering the symbol per error with no text classified and
no `String` built. The kind's *own* name is fixed the same way: `KIND` holds one
`StaticName<TypeSymbol>` per `KErrorKind` surface name in that file, read by both the lowering and
the prelude registration that mints the `KError` union's members, so the two ends cannot drift.

A name the machine *binds into a program's scope* is fixed in source the same way. `it` — the
scrutinee binder every `MATCH` and `TRY` arm opens — is a `StaticName<ValueSymbol>` in the
`MACHINE_BINDERS` group ([binder.rs](../src/machine/model/binder.rs)), which collects every binder
the surface fixes rather than spells: the arm binder beside an `OP` body's `left` / `right` and a
`UNARY OP` body's `operands`. The builtin that installs one reads it back from there and hands it
to `record`. They are value-class like a parameter slot but are not ones: no signature registers
them, and a form binds under the symbol the static already holds, so an arm taken or an operator
applied hashes no text.

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
`KObject::Wrapped`'s `type_id` is the interned handle of the member whose name the declaration
minted, so constructing a variant bumps no discriminant bytes into its region and a `MATCH` arm
head selects by comparing that handle against the member its own head named
([value-substrates.md](value-substrates.md)).

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
`KeywordSymbol` and interns it in the same step, and the part carries that symbol alone —
`ExpressionPart::Keyword(KeywordSymbol)`, no spelling beside it
([ast.rs](../src/machine/model/ast.rs)). Nothing downstream re-hashes: a node's bucket key, a
signature element, an operator chain's cached registry probe and every keyword comparison read
the symbol the parse already minted, and the fixed tokens the machine itself compares against
(`AS`, `->`, `_`, the binder's key specs, the reserved operator names) are `StaticName` memos
minted once per process. That placement is not convenience — `Symbol::of` is a BLAKE3 hash, and
a keyword sits on the hot dispatch probe path, where paying one per keyword per call is exactly
the cost a parse-time cache exists to remove.

A keyword spelled in **Rust** source rather than in koan source — a builtin signature's fixed
tokens — converts at the draft door, `SignatureElement::keyword`
([signature.rs](../src/machine/model/types/signature.rs)): it normalizes the spelling (a
lowercase-bearing token uppercases, so a builtin drafting `let` keys the bucket `LET` computes)
and then classifies and interns it. `ExpressionSignature::mint` copies the settled element, so a
keyword element is hashed where it is written and nowhere after, and a signature's elements run
is a slice copy into the region rather than a text re-home.

An operator chain's registry probe is minted from **symbols too**, never from a join of
spellings. `KeywordSymbol::of_run` takes the run of operator symbols a chain names, sorts them by
symbol bits, dedupes, and hashes their digests through the same `Symbol::of_hash` funnel every
other mint ends in; a `GROUP` registers its powerset of keys through `declared_run`, the same
constructor plus a recorded rendering of the joined spellings for the diagnostics that name a
probe. So a registered key and a live chain's probe agree by construction and no probe path
touches text ([operators.md](operators.md)).

The **type** vocabulary converts there too, on the same reasoning. Where the parser classifies a
token as Type-class it mints the token's `TypeSymbol` through `declared` and the part carries the
symbol alone — `ExpressionPart::Type(TypeSymbol)`, no surface text beside it. Every later reader
carries that symbol through: the bind seam's `UnresolvedType` carrier for a type *reference*, the
`Held::Name` carrier for a type-declaring builtin's binder name, the type-side lookup ladder
(`Scope::resolve_type_with_chain`), `elaborate_type_identifier`, and a variant-tag match.
Because the symbol is lifetime-free and `Copy`, a type name
crossing a region boundary is copied rather than re-bumped, and a name that must be *printed*
resolves back through the interner.

The **value** vocabulary converts at the parse boundary too, and for the same reasons — the two
name classes are mirror images. Where the parser classifies a token as neither keyword-class nor
Type-class it mints the token's `ValueSymbol` through `declared`, and the part carries the symbol
alone (`ExpressionPart::Identifier(ValueSymbol)`, `FieldSlot::Name(ValueSymbol)`). Every later
reader carries it: the bind seam's `Held::Name` carrier, read back through
`BoundArgs::identifier` (value class only) or `BoundArgs::name` (either class), the
value-lookup ladder (`Scope::resolve_value_delivered`, which takes a `ValueSymbol` rather than a
spelling), each binder builtin's own name read, and the statement's `StoredBinderKey`, which
mints nothing. Because no lookup re-derives a digest, a name spelled once and read
many times is hashed once, at the parse.

`Held::Name` is where the two vocabularies meet: it carries a
[`BinderSymbol`](../src/machine/model/labels.rs), the class taken from the part variant the
parser assigned, so one carrier serves every name-capture slot and no consumer re-derives a
class from a rendering. See
[tokens.md § A binder position is a name](typing/tokens.md#a-binder-position-is-a-name).

The consequence for the surface is that neither name class admits a bare probe. A binder builtin
that needs the spelling back — for a diagnostic that quotes the name — renders it out of the run's
interner rather than holding text beside the symbol.

## Probes never intern

A runtime string probing a record computes `Symbol::of(text)` and searches the
record or substrate directly. A miss is a lookup miss; the interner is never
consulted and never written.

The **derived-symbol door** is the exception, and there is exactly one in the whole
tree: [`classify_derived_field`](../src/builtins/attr.rs), which the one member name
that reaches `ATTR` as text rather than as a parse-minted token funnels through — the
runtime string the dynamic `ATTR <s> <field :Str>` overloads read. A bare field token
of *either* class never reaches it: the `field` slot is a `NameToken`, so the token
arrives already carrying the class the parse assigned it, and nothing on that path
renders or classifies text. It classifies through `BinderSymbol::declared`, so a spelling read
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
paths stay total — that is `LabelInterner::render`, the total form for a caller that
needs an owned `String` (`resolve` is its fallible twin). The **ordinary door is
`display`**: the same read as a `Display` view, so a message that names a label writes the
recorded text straight into the message's own buffer with no `String` in between, and a
`format_args!` position that carries a label passes the view rather than a rendered
fragment. A diagnostic built that way costs its own buffer alone.

Beside them sits **`compare_texts`**, the order door: two symbols compared by their recorded
text under one borrow. The render arms that print name-sorted — a record value's fields, a
signature's schema members — consult it instead of rendering first and sorting the fragments,
so a sorted arm stages symbols and the comparison touches no allocation. A symbol this run
never recorded compares as the same placeholder `display` writes for it, so an unrecorded name
holds a fixed position rather than a hash-dependent one.

Rendering happens **where a diagnostic is built and nowhere earlier**. A lookup that
misses carries the symbol out rather than a message — `Resolution::Unbound` and
`TypeChannel::Unbound`
([resolve.rs](../src/machine/execute/decide/resolve.rs)), the dispatch walk's dead lean
and its `DispatchOutcome::UnboundName`
([resolve_dispatch.rs](../src/machine/execute/decide/resolve_dispatch.rs)), and
`TypeResolution::Unbound` ([resolver.rs](../src/machine/model/types/resolver.rs)), whose
one wording every unbound type-name arm shares through `unknown_type_name`. That matters
because the value-side miss is not an error arm at all: it is the ordinary "this bare name
is not a value" fall-through every keyworded dispatch takes, so a program that never
prints a name never renders one. A binder builtin reads its own name back the same way —
on the arm that quotes it, not before.

Because the surface parts of an expression carry symbols rather than text, the surface
rendering of a part, an expression or a trace frame resolves through the run's registries, and so
do the walkers that quote a name in a diagnostic. A field or parameter list is the shape
to read that against: `parse_pair_list` and
`parse_type_tag_names` — the pair-list door and the variant-tag pre-scan
([triple_list.rs](../src/parse/triple_list.rs)) — take the run bundle, read labels through it, and
hand each name on as the symbol its own token minted, rendering only inside the message a rejected
or duplicated name raises. The name a declaration *keeps* is never rendered — text appears on the
error path and the print path, nowhere else.

### The surface-rendering family

One tier up from a label sits the rendering of a whole surface value — a `Held` or `Carried`
cell, a `KObject`, a `KType`'s name, and the `Part` trait's per-part rendering that
`ExpressionPart` and `WorkingPart` implement. Each is a `Display` family with the same three
spellings:

- **`write_summary(f, …)`** (`KType::write_name` for a type) is the one place the arms are
  written. Nodes are read in place and children recurse into the *same* formatter, so a nested
  value costs the caller's buffer and nothing else — no per-child `String`, no `Vec` to `join`.
  It is the required method on [`Part`](../src/machine/model/ast/shape.rs), where a
  view-returning signature would need an associated type per implementor and a formatter
  argument stays object-safe.
- **`summary(…)`** (`KType::display_name`) is the `Display` view — the `format!` argument a
  diagnostic passes, so a message that names a value builds one buffer: its own.
- **`summarize(…)`** (`KType::name`) is `summary(..).to_string()`, for the callers that *keep*
  a rendering rather than write it somewhere: a `KError`'s stored `expr` / `got` text, a trace
  frame, a deferred return's stored surface.

The `Part` family renders through the whole run bundle rather than the interner alone, because it
names types rather than spellings. A [`WorkingPart`](../src/machine/model/ast/working.rs) summary is
read inside a diagnostic explaining a dispatch outcome, so every argument slot renders **the type
dispatch matched it on** — not the value the slot holds, and not the spelling that produced it. A
slot is matched on its type alone, so the type is the whole of what the decision saw. What pays for
dropping the echo is the site: `KErrorKind::DispatchFailed` and `AmbiguousDispatch` each carry the
offending expression's resolved location and render it beside the expression, in the same
`at <path>:<line>:<col>` shape a trace frame uses. Owning it rather than borrowing an enclosing
call's frame is what makes a top-level statement — which nothing encloses — locate at all.

The arms mirror `KType::accepts_working_part`, which is what decided the outcome.
`KType::slot_ktype` answers an unevaluated AST slot — the inverse of `KType::accepts_part`, read as
a function instead of a predicate — and `Carried::ktype` answers a resolved `Spliced` cell. Neither
reports a stand-in for the slot's *shape*: a container literal types its own contents, recursing
through `slot_ktype` and joining the results with `TypeRegistry::join_iter`, the same join
`KObject::list_of_held` applies once those elements have evaluated, so `[1 2 3]` answers
`:(LIST OF Number)` whether or not it has run yet. One consequence is that a slot reads the same
either way: `RUNLATER 3` and `RUNLATER (1 + 2)` both render `RUNLATER Number`, because dispatch saw
the same type in both.

A bare name reports the type of the value it is *bound to*, which takes one step of care. The
pre-dispatch scan resolves every bare-name part into `bare_outcomes`, a parallel array beside the
parts run, and the success path splices the picked wrap set back into the expression. A miss has no
pick, so `splice_resolved_names`
([keyworded.rs](../src/machine/execute/decide/keyworded.rs)) splices the lot before the diagnostic
summarizes: without it the miss would render the parts run as it stood *before* resolution, where a
name is still its own token, and report the token's type rather than the bound value's.

Two positions stay off the type rule. A binder's name slot (`binder_name_slot`) keeps its spelling
through `WorkingPart::write_spelling`, because nothing dispatches on it — it is the name being
installed, and naming its type would render every `LET` alike. And the scheduler's own unfilled
arms — a synthesized node it will dispatch, a hole awaiting a sibling's carrier — hold no value
yet, so only an `Any` slot admits one and none of them narrowed the candidate set that missed; they
render `<staged>` rather than name a type they do not have.

The two expression families differ in signature here: `ExpressionPart`'s inherent `summary` keeps a
bare `&LabelInterner` and renders surface spelling, which is what a *parse*-time shape error wants
(a record literal's rejected field name, in
[dict_literal.rs](../src/parse/dict_literal.rs)) — and parse renders it while still *filling* the
interner a run frame has yet to adopt, so the bundle is not available to it. Its `Part` impl narrows
the bundle down to that interner.

Two consequences follow from the streaming shape. A renderer cannot inspect the text it has
already written, so the parameter-record sigil convention — a leaf type surface takes a `:`
prefix, one that already opens a sigil does not — reads the node's shape through
[`KType::surface_opens_sigil`](../src/machine/model/types/ktype.rs) rather than the rendered
string, and that predicate's arms are exactly the arms `write_name` opens with `:(` or `:{`.
And bytes that must *exist* rather than be written into a message are rendered straight into a
region: `PRINT` hands its view to
[`BumpAllocator::text_from_display`](../workgraph/src/witnessed/bump.rs), so the slice written
to the output sink and the bytes the returned `KString` carries are one allocation in the
step's own destination region.


## Open work

None tracked.
