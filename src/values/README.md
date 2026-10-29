# Values

Koan's data values and the per-dispatch expression form, laid down in a cell's
region through [`memory`](../memory/README.md)'s shapes and nothing else.

A value is what a container cell holds, what a binding holds, what a spliced
slot holds and what one cell delivers to another. This module says what one is,
how it is built, what type it has, what moving it costs, and how it moves; it
says nothing about who asks. Dispatch, delivery and name resolution are the
layers above, and they call in here.

## What a value is

A [`Value`](../values.rs) is one `Copy` word of 24 bytes: a number, a bool,
null, a string borrowed where its bytes live, or a borrow of a **per-kind
resident struct** —
[`List`](list.rs), [`Dict`](dict.rs), [`Record`](record.rs),
[`Tagged`](tagged.rs) or [`TypeValue`](type_value.rs) — or a **knot member**.
`Value` is the sum and the per-kind structs carry the methods, so a consumer
that only reads lists names `List` and never matches on every kind.

**A knot member is a parameter.** A group of values that refer to one another —
mutually recursive functions, a ring of tagged values, a list holding a
function that captures it — is born together as one
[knot](../memory/README.md#the-knot), and a function holds the environment it
captured, which is the [scope layer's](../scope/README.md#three-tiers), which
`values` may not name. So `Value<'cell, X>` has one arm, `Knotted(X)`,
over a type parameter a layer above closes — [`knot`](../knot/README.md)
closes it with a sixteen-byte `(knot, index)` member, so the word stays at 24
bytes — and a second, `Retyped`, a borrow of a [`Retyped`](retyped.rs) header
over a bare member ([below](#the-type-memo-and-satisfies)). Every `as_*`
accessor reads through a header to the bare member. `values` states what it asks of `X` as a trait pair
([values.rs](../values.rs)): per value, `Knotted` — a `Copy` type whose
equality is node identity, with its memoized type handle, its knot's weight,
the fellow member an edge of its own knot names, what the node holds, and the
values its knot holds beside the knot's digest over theirs (`held` and
`digest_held`); per family, `KnottedFamily` — the member at each region
lifetime, and the copy of its knot from one to another. The parameter defaults to the uninhabited
`Nothing`, whose family is `NoKnot`, so a value spelled without it holds no
knot member and every arm that builds one is unreachable. Every container, the
working expression, and every door and relation over them carry the same
parameter.

**What a member holds** is the one total answer `Knotted::resolve` gives, a
[`Resolved`](circular.rs): a **function**, as its identity, the solution it
was instantiated at where it is an instance of a quantified function, and its
closure bindings; a quote's **code**, as a `CodeView` of its body as written and the
bindings its names carry; a **module** or a **barrier**, both opaque to
`values` — a module carries no type this module names; or a **data node**, a
[`Circular`](circular.rs). A function's identity is a `usize` the layer above
derives from its body shape, since `values` names no shape. `Value::as_callable`
answers for a function and a barrier, which calls the same way,
`Value::as_code` for code, `Value::as_module` for a module, `Value::as_opaque`
for the two opaque arms — which is what equality refuses — and
`Value::as_circular` only for the data node, so no arm's meaning rests on an
invariant `values` cannot check. A data node is a list, dict, record or
tagged resident whose cells are [`Link`](link.rs)s instead of value words: a
link is a value word or an `Edge` naming a sibling node of the same knot, since
a sibling has no address until the knot is tied. A link is read only through
the member holding it, which resolves an edge to `Value::Knotted` of the
sibling. A quote is a knot member for the same reason a function is: its `$`
names bind values, and one may name a fellow member, as
`LET echo = #(PRINT $echo)` names itself. The four residents take the cell type as a parameter defaulting to the
value word, so their accessors and deep-copy doors are written once; each has
a `linked` door that lays a data node down under a memo its caller already
derived, and the plain doors stay on value cells. A function's closure
bindings are the same `Link` run, and so are a quote's bound and supplied
names.

No type handle rides in the word. A `KType` is a `u128` aligned to 16 bytes, and
one inline would more than double every cell; every handle lives in the resident
struct an arm points at, and a leaf's type is a constant.

Every composite is **born through a door that takes the region's `Writer`** —
the brand-confined capability `cellgraph` hands a step or a build closure — and
comes back as a `&'cell` borrow co-located with everything it points at. There
is no other constructor, no reference count, and no per-value reach
description: a value built in a step is a plain reference whose reach is the
executing cell, and a value crossing a step rides the substrate's carrier,
[`ValueCarrier`](../values.rs), `memory`'s `Ready` bound to
[`ValueFamily`](../values.rs) over a knot-member family. Every resident struct
is `Drop`-free, which `Writer::fill` asserts, and every knot member is `Copy`,
so a region releases each whole. Every door lays its
struct down through `memory`'s `resident` and its runs through `collect`, the
two shapes derived from `Writer::fill`; `text` is the one helper here, a string
value over `Writer::text`.

A tagged value is the one nominal wrap — a newtype construction, a
construction through a family, a union variant, a lowered error — and its
identity *is* its type, so no tag symbol rides beside the payload.
`Tagged::hold` keeps every layer of a payload that is itself tagged;
`Tagged::peel` replaces one, so a re-tag never nests.

**A construction has one rule.** [`construction`](admission.rs) takes the
handle a construction's head names and the type of its payload, and answers the
identity the tagged value takes, or a `ConstructionRefused`:

- a **newtype** gives the head itself, or `Misfit` when the payload's type does
  not satisfy its representation;
- a **family with a representation** — a one-parameter `NEWTYPE` family, or a
  parameterized union's variant ([families](../elaborate/README.md#families)) —
  solves the representation's parameters against the payload's type and gives
  the family applied at the solution: `Boxed` over a number gives
  `:(Boxed {Type = Number})`. The solve takes the least solution, so a
  parameter the payload does not reach is `Never`, and a construction of
  `Result.Ok` over a number carries `:(Result.Ok {Ok = Number, Error = Never})`,
  which lies under every `Result` whose `Ok` admits a number. A payload the
  representation cannot be solved against — a structural misfit, or
  contributions to one parameter joining outside its bound — is `Unsolved`, naming the
  family and the payload's type;
- any other head is `NotConstructible`: a scalar type, a family that constructs
  nothing, and an applied family, whose arguments a construction does not take
  from its head.

`Tagged::construct` is the checked door over it;
`hold` stays the raw wrap for a union variant, a lowered error and a retype.
Every construction goes through the rule — an ordinary one through `construct`,
a knot's tagged node by the tie over the payload type it derived — so there is
nothing to keep in agreement.

**A seal is the second checked door.** [`sealing`](admission.rs) is what an
opaque view's barrier goes through
([members are born coerced](../knot/module/README.md#members-are-born-coerced)): a
head parameter records no representation for `construction` to check a payload
against, so what is checked instead is that the identity is a **carrier** — a
`Parameter` keyed on content — and that the payload satisfies what the source
binds that parameter to. It answers the carrier as the identity, or a
`SealRefused`: `NotACarrier` for an identity that is no carrier, `Misfit` for a
payload the source's binding does not admit. `Tagged::seal` is the checked door
over it, and it `peel`s, so a sealed member takes the carrier as its one tagged
layer rather than a second one. Sealing happens where a view is built, never
where a koan program writes a construction; a value crossing a barrier inwards
that is not sealed under the carrier is `NotSealed`.

**A seal is read through nowhere outside its view.** A sealed value stays a
`Tagged` wrapper — a scalar carries no type of its own, so the carrier rides on
the wrapper — and rendering keeps it. A carrier lies under `Any` alone
([the order](../../lattice/src/types/relations.md#the-relations)),
whatever bound the member declares: the bound decides which modules fit the
signature, and nothing more. So no slot but one naming the carrier takes a
sealed value, equality compares it by identity as it does any tagged value,
and it keys no dict. A 5 sealed behind a member bounded by `Number` neither
equals 5 nor passes a `Number` slot. Only the view's own functions, behind
their barriers, see the payload.

## One lifetime

A value borrows at one lifetime, `Value<'cell>`: what a writer laid down in a
cell's region, following
[cellgraph's contract](../../cellgraph/README.md#the-contract-four-embedder-types).
No arm holds program storage. A quote's body lives there, but a quote is a knot
member, and what a member holds of program storage — a function's body shape, a
quote's body and code shape — sits inside the member, at the lifetime the layer
above closes the parameter with.

So **every value copies**: every region part has a deep copy, and a knot member
is rebuilt by its family, which embeds its program storage verbatim, so the copy
is total. No AST node is ever homed in a cell.

## The type memo and `satisfies`

Every composite stores its type as a memoized [`type_lattice`](../../lattice/src/types/README.md)
handle, computed in the pass that lays its cells down: a list the union of its
cells' filled types (`Never` when empty), a dict the union of its keys' types
beside the union of its values' filled types, a record the record type of its
fields in written order, a type value `OfKind` of the kind of the type it names,
a tagged value its identity, and a knot member reports its own, or its header's.
So a list holding a number and a type memoizes `List<(Number | ProperType)>`.

**A container fills its records** ([fill.rs](fill.rs)). At each position below
a container — a list's element, a dict's value, a record's field, across a
union's members — each record gains, at `Null`, every field another record at
that position names, so no record lies under another by width and the union
keeps both: `[{x = 1, y = 2}, {x = 3}]` is a
`List<({x :Number, y :Number} | {x :Number, y :Null})>` and prints
`[{x = 1, y = 2}, {x = 3, y = null}]`. The fill stops at every other type
node — a scalar, `Any`, a tagged value, a function, a type, code, a module —
and never fills a dict's keys. It is computed from the cells' types alone, as a
**merge**: the types reaching one position form a sorted, deduplicated group,
and a group of fewer than two maps each type to itself, since a container's own
type is already filled. So the merge descends only where two distinct types
meet, and `[x, acc]` costs O(1) however deep `acc` is. Each group's results are
memoized in the [lattice's memo](../../lattice/src/types/identity.md#downstream-memos),
along with the results' own group as mapping to itself, and the merge runs over
an explicit stack. **The fill writes no cell**: the door reads a field seen at
exactly `Null` as `null` without its cell, and a part whose own type falls short
of a member of the type it is read at is taken at that member once its own type
is filled against it, at every depth. A record does not fill its own fields;
only a container fills.
`Value::ktype` copies that handle or names a leaf constant, and reads no
type tier and walks nothing. It answers a
[`DeclaredType`](../../lattice/src/types/identity.md#typed-handles): a concrete `KType`
for every value but a quantified callable, which answers its `Scheme`. A
quantified callable is read only at the head of a call or as a `MODULE` or
`GROUP` member's binding, and the load makes every other read of one an instance with a concrete
type ([resolution](../scope/README.md#resolution)), so every other reader —
a container's type, a construction, a diagnostic — takes
`Value::concrete_ktype`, which names that rule where it narrows. A key's
candidates — the functions a `USING` hole or an `EVAL` offer gathers at one
key — are laid down by `List::of_candidates`, typed `List<Any>` without reading
any function's type: dispatch alone reads such a list, each function by its own
type, and a quantified registration's scheme enters no list type. A quote's type is its carried type, memoized on its
node: its [code kind](../../lattice/src/types/vocabulary.md#the-code-family), read off its
body as written ([`KExpression::code_kind`](../parse/README.md#the-ast-borrowed-copy-and-splice-free)),
needing the `\` names no binder in its code fills
([code parameters](../scope/README.md#code-parameters)): `#(y)` is a `Name`,
`#((y))` an `Expression`, `#(PRINT \y)` an `:(Expression NEEDING #[y])`. A
container of quotes is an ordinary container, typed by its elements' types as
any other is.

**A data node's memo is exact and finite.** The lattice has no structural
recursive type, so recursion in a value's type goes through a declared memo: a
function's is its signature type and a tagged node's its newtype, both known
before the knot exists. A container node's memo is derived by the one rule
of its kind — `list_type`, `dict_type` or `record_type`, which the plain doors
use too — from its cells, an edge contributing its target's memo, so
the layer that ties a knot derives it — and refuses a cycle of containers
alone, which no finite type describes
([the tie](../knot/README.md#the-tie)). `satisfies` over a circular value
is therefore the same one relation against that memo.

A type check against a value — [`satisfies`](admission.rs) — is therefore one
lattice relation between the slot and that handle: *fits*, read from the
slot's side (`satisfied_by`). A slot whose variables a call solves is the
caller's collector to admit through, never `satisfies`. Nothing descends into a
value to type it, so a value's precision is whatever its type says, and **an
ascription changes it** — a `:!`, a parameter binding its argument, a frame
returning under its contract. `Value::retyped` reads the declared type member
by member (a union's members, or the type alone): the value takes the meet of
what the members of its own kind take it at — a list, dict or record node for
a container, a node naming its constructor for a tagged value, a function type
or an expression shape for a function or a barrier, a code kind for a quote's
code, a kind for a type value, each it lies under, and for a module each
signature application it fits, its head parameters pinned at what *fits*
solves (`pinned_at_fit`, over `fit_solution`, the solve the view door reads
too) — and keeps its own type where there is none, so a
list ascribed `Any` stays as precise as it was. A record seen at a record type
keeps at `Null` each field its own type lacks or has at exactly `Null`:
`{x = 1, y = null} :! :{x :Number, y :(Number | Null)}` carries
`{x :Number, y :Null}`. The target depends on the
value and the type alone, never on member order. Which types re-stamp a value
at themselves is one predicate in [surface.rs](surface.rs), `retyped_kind`:
each of those, save `Module` and `Code`, which every module and every quote
satisfies and which re-stamp nothing, as `Any`, `Value` and `Never` do. The
load's static types of a retyped value (`retyped_to`) and of one at most a type
(`under`, `unknown`) live beside it and read it, and so does
`retypes_by_header`, which says where a retype lays a header and where the
load's content tree takes a retype node, so the load calls a retype
exact exactly where the run makes it so — save a signature type leaving a head
parameter unpinned, which each run pins afresh at what its module fits, so the
load calls it at most that type. So is a record type with a field `Null` lies
strictly under: the load reads a retype to it as at least itself with each such
field at `Null`, and at most itself.

A plain value takes the target as its handle over the same shared runs, and a
type value its kind beside the same named type. **A knot member takes a
header.** Its type lives in the node every holder shares, so the node cannot be
re-stamped; a retype lays a [`Retyped`](retyped.rs) down instead — the bare
member beside the type it shows, O(1) — and `Value::ktype` answers that type.
The header holds the member, not a value, so a retype of a retyped value is one
new header over the same member, and one back to the member's own type hands
back the bare member. Only a callable's header may carry a scheme. A data node
under a header is still a knot member: it reads, compares and renders through
the door at the header's type and closes its cycle as a cycle. The retype
restamps the top value alone, and its cells keep their own types; a read
carries it down.

**A value's type is its surface.** Every read of a record, list, dict or tagged
value — a field read, `FROM`, `USING`, a frame binding its arguments, module
coercion, rendering, equality, the deep copy — goes through one door,
[surface.rs](surface.rs), which sees only the fields the carried type names and
hands back each field, element or entry retyped to its type there, and a tagged
value's payload retyped to its identity's [`representation`](admission.rs): a
newtype's, or a family application's with its arguments substituted. After `{x = 1, y = "a"} :! :{x :Number}`, the value has no field `y`
to any reader and prints `{x = 1}`, though its cells still hold `y`; a
`LIST OF (LIST OF Any)` retype hands back each inner list as a `LIST OF Any`;
and `(Point {x = 1, y = 2, z = 3})`, a `Point` over `{x :Number, y :Number}`,
prints `Point({x = 1, y = 2})`. An identity with no representation, such as an
opaque view's carrier, hands its payload back at the payload's own type. A retype
walks nothing, and a nested part obeys it all the same, so downstream dispatch
sees the contract at every depth rather than the contents' incidental
precision. An element is read at the type its container names, filled: a
list ascribed `LIST OF {x :Number}` reads each element at `{x :Number}`.

A read carries a `Seen`: a value beside the type the read sees it at — its own
memo at the top of a read, and below it what the holder's seen type names for
the part there, narrowed as a retype narrows. A value walk can meet a
quantified callable — a closure's capture, a candidate list's cell — so a
`Seen` holds a `DeclaredType`, and a callable is seen at its own scheme; a value
of a kind is always seen at a type. `Seen::surface` opens a container
or tagged value at that type as a `Surface`, whose parts are each seen at their
type there. A reader that only inspects a part — equality, rendering, a name, a
function to consider — takes its value and writes nothing; a reader that hands a
part on as a value of its own — `ATTR`, `USING`, a frame binding an argument,
module coercion — restamps it at its seen type (`Seen::restamped`).
`Value::retyped` is that pair at the top: the value seen at the declared type,
then restamped. A resident's value-cell runs are private to `values`, so no
reader outside it can step past the door; a data node's link runs stay public
to the knot layer, which ties them. A construction holds its payload verbatim,
at the payload's own type: the door hides what its representation does not
name, and a copy drops it.

The same module answers the question for what is not yet a value.
`admits_part` checks a raw AST part by shape, since an unevaluated literal has
no type memo: a container literal admits by its elements — a list by each item, a
dict by each key and value, a `_` key admitting any key type, a record by each
field the slot names, each record filled against the others at its position as
the run fills it, so a field another record there writes is admitted where its
slot admits `null` — a code kind on a part whose own code kind lies under it,
a bare group being code of its own kind as a quote is, a union on any member, a
family top — `Value` or `Code` — on any concrete type of its family,
a kind slot takes a type token only for `ProperType` and `AnyType`, a
quantified slot takes what its bound takes — a slot bounded by `Value` refuses a
quote — and a nominal, function, signature or shape
slot takes no raw part at all. So a type token is taken by `Code` (as a
`TypeNameToken`) and by `Type` alike. `part_ktype` is its inverse — the type dispatch
matched a raw part on, and the one a diagnostic renders — and the two agree for
every part shape. `admits` routes a working part to one or the other.

## Weight

Every composite stores its [`Weight`](weight.rs) beside its type: the bytes a
total rebuild at a destination writes, saturating. A composite weighs its own
resident struct plus every cell it lays down, and a cell weighs a whole `Value`
word plus whatever that word points at in the region — a string its bytes, a
dict key its key word and bytes, a record name its symbol. A tagged value holds
its payload word inline, so it adds only what the payload points at. A link
weighs its word plus what a value word points at; an edge points at nothing. A
knot member weighs what its own rebuild writes, which its layer memoizes — the
whole knot it sits in, since a member copies by re-tying its knot, and a data
node's resident weighs only its own struct and links. Program storage a member
holds weighs nothing past the pointer. A crossing reads the weight off the
value rather than walking it; a plain retype shares the runs, so it shares the
weight, and a header weighs its own struct beside its member's weight. So a value whose type hides some of its cells weighs
them all, while its copy lays down and weighs only what the type shows: the
[verdict](#crossing) may over-price such a copy, and never under-prices one.

The weight's top two bits are a second memo, [`MemoBits`](weight.rs), which
the byte count saturates below and which its equality and order never read:
**root-closed**, every type the value reaches is the root's, and **settled**,
every type it reaches resolves in the root or its home region's branch. Each
header holds its weight in a `Cell`, so an exit can set the settled bit
through the shared borrow a value is read by; a knot keeps its bits on its
root node. The doors compute both where they lay a value
([settled.rs](settled.rs)), and an adoption does not enter a part with either
bit set ([type tiers](../../design/type-tiers.md#an-adoption-stops-at-a-settled-part)).

**A hidden field never shows.** A part *hides* a field when it stores a cell
for it that its view — the type the door reads it at through its holder — does
not name, or names at exactly `Null`. The weight's third bit, **hides**, is set wherever the value or a
part below it may hide a field. Types alone decide it, so it may be set where
nothing is hidden, which costs only a visit, and it is kept without visiting a
value ([hiding.rs](hiding.rs)): a scalar, a function, a type, code and a module
hide nothing, a composite built from parts ORs their bits, a restamp from one
type to another ORs in a walk of the two types alone, memoized in the lattice
(`hides_more`), and a holder rebuilt over a copy sets it where a cell it keeps
still hides a field at its view there, or, for a record, where it stores a
field its own type does not name. Where a value with the bit set comes to be held at a new
view — built into a container, or restamped by a binding, a handing on or
`:!` — each part whose new view names a field its old view hides, or names at
exactly `Null` a field it stores at another type, is copied shallowly without
that field, over the same cells, and each holder on the path above it is
rebuilt over the copy. The copy and each rebuilt holder take their own type
narrowed by their old view, less the fields they drop: the type names what each
holds and no `Null` a holder's fill gave it, so no type names a field a copy
below it dropped, and a value reads alike whether or not a part of it was
copied. So `xs = ([{g = 2, h = 1}] :! :(LIST
OF :{h :Number}))` retyped to `LIST OF Any` prints `[{h = 1}]`, and no record
stores a value under a field its view has at exactly `Null`. A value without the
bit is never visited. A knot member is never copied: the door reads it at its
holder's type.

## Crossing

Moving a value between regions is one verb over the substrate's two placement
doors ([crossing.rs](crossing.rs)): `cross` builds a carrier's value into
another cell's region and hands back the carrier resting there, and
`cross_here` builds it into the executing cell, where it is a plain `'here`
value the step may embed or its continuation capture. Both price the operand at
its weight, and both build through `cross_view`, which turns one crossed operand
into a value at the destination's brand: a **pinned** operand arrives there and
embeds as it is, and a **copied** one is rebuilt by the deep copy — region
parts written again through the destination's writer, and a knot member's whole
knot re-tied by its family. The deep copy reads through
[the door](#the-type-memo-and-satisfies) as every reader does: each part holds
only what the type it is seen at names, and is weighed by what it lays down, so
a part a retype hid is dropped, and a field a fill gave a record, which it does
not store, stays unstored. A list, dict or record is laid down at its own type
narrowed by the type it is seen at, as [a hiding part's copy](#weight) is, so a
copy reads as a pinned source does under every later type, a widening `:!`
included; a tagged value is laid down at the type it is seen at. A knot member copies
with its knot, bare or under a header, and one seen at a type other than its
own arrives under a header at that type, which the copy adopts into the
destination first. The crossing doors therefore take the
destination's type chain, which the copy reads its types in, and the
scheduler's step doors that cross a value build that chain inside the build,
over the graph's type tiers and the build's writer and header.

**A crossing carries types with the value.** A type the run builds lives in the
branch of the habitat its value is laid down in
([storage](../../lattice/src/types/identity.md#storage-a-root-and-its-branches)).
The branch is kept in the habitat half's header, whose record is `scope`'s
([`RegionHeader`](../scope/header.rs)): beside the branch it holds the
decision memo of the cells whose results are built in the half
([the build module](../build/README.md#activations)).
`values` cannot name that record, so a [`Half`](habitat.rs) holds its header
door projected to the branch's slot ([`BranchSlot`](habitat.rs)): any header
family that projects to it serves, and `values` reads and lays the header
through that slot alone. Projecting lays nothing, so a half that never interns
lays no header; its first intern lays the record and the branch.
What the build arena's books are keyed and bounded by lives in
[settlement.rs](settlement.rs): a settlement's key, a seek's binding image, the
budget, and `ArenaRef`, the opaque handle naming an arena node above the
scheduler — a slot and its generation — opened only by `values` and the
scheduler
([the build module](../build/README.md#references-between-frames-and-the-arena)). A
[`Chain`](habitat.rs) carries a pairing brand naming the writer it serves
beside the brand its tiers sit at, so a value door handed one half's writer and
a chain paired to another does not compile. [`Types`](habitat.rs) is the chain
whose tiers sit at its writer's own brand; a caching body's chains lay in a
scratch tier and are paired to its region writer. What a reader sees of a value is bounded only by the value: under
`Any`, a union, a function type or a carrier a part keeps its own memo, so the
types a value reaches are the closure over its parts' memos. A copied operand
therefore adopts each part's own memo into the destination's branch before the
type it is seen at is read off it — re-laying, from the home's branch, what the
destination lacks — and adopts nothing for a part the seen type hides; a type
value's payload, and every type a knot's nodes carry
(`KnottedFamily::carried_types`), are adopted before they are laid. A pinned
operand is not walked. A pinned birth links the destination's branch to its
home's — one pointer a read by handle follows and caches — where the
destination has laid no branch: a spawned child always, and a hop's successor
or what a root work leaves when it lands in a new cell. A pinned result or
view, and a pinned birth into a branch already laid — a tenant's, which is its
host's — merges its home's branch into the destination's, with each branch the
home links up to one the destination already reaches, a cost in the types
those branches hold, never in the value.

**A caching body adopts at its exits.** Its values lie in its region and
their memos in a scratch tier, so before a value leaves the body
[`adopt_value`](crossing.rs) walks it as the crossing check does — each part's
memo, each type value's payload, each type a knot carries and every value it
holds — and adopts every type into the region, reading the whole chain the
body ran on. A part whose [memo bits](#weight) meet the contract is not
entered and hands over nothing, its own memo included, and the exit marks every
value it did enter settled after, so a value handed on through a loop is walked
once. A header, whose bits are its member's, hands over its own memo. The walk reads the top value's bits before it lays
anything and lays its stack, knot set and settled list in its caller's step
scratch
([type tiers](../../design/type-tiers.md#an-adoption-stops-at-a-settled-part)).
A test build audits each part as the walk skips it, walking it whole.

**A debug build checks every crossing.** After each crossing door, a debug
build asserts that every type the crossed value carries — each part's memo,
each type value's payload, each type a knot carries — resolves in the
destination's chain, reading and caching nothing. A copy, which walked the
value already, is checked whole; a pin, which a release build never walks, is
checked over the first sixteen parts a depth-first walk reaches, which finds a
crossing that missed a tier unless every type that tier holds lies past them.

**The deep copy runs over an explicit stack**, in a bump of its own, since no
step scratch reaches a birth's crossing; so a value of any depth copies without
growing the call stack. A composite is a frame whose children are copied first,
and it is laid down once they are all finished. A knot member is a frame over
the values its family **lists** (`KnottedFamily::held`): the copy copies each of
them, then hands the family's `copy_into` a cursor that answers each value it
asks for with its finished copy, in the order `held` listed them, so the knot is
tied once, after everything it holds has a copy. A data node lists and rebuilds
through `Circular::held` and `Circular::copied`: each value link through that
copy, each edge verbatim — an edge names a node by index, so it means the same
node in the copy — and its memo and its weight's bytes carried over. A composite copy is
laid through the doors that compute its memo bits over the destination's
chain, and a re-tied knot's bits are stamped there. A re-tied knot keeps
every cell its nodes hold, a field a tagged node's representation hides
included, and the door still hides it there.

**One copy per knot per placement.** A copy keys every knot it has re-tied by
the knot's root member, so a second reference to that knot is the copy's member
at the same index: two members of one knot crossing together arrive as members
of one copy of it. A plain value reached twice still copies twice, and a
value's [weight](#weight) still counts a knot once per reference to it.

The copy is private to the crossing, and its callers are two public doors,
because a placement a layer above builds over values hands them the views.
`cross_view` rebuilds a value that is itself the crossed operand — a
scheduler's result built in its home. `copy_severed` rebuilds values held
inside a *copied* operand of another family — a birth that carries values, such
as a call's callee and arguments woken into its frame — at the operand's
severed brand, all through one copy, so the values of one birth share each knot
they reach. Each takes a `CrossedOperand`, which only a priced placement
mints — its arms are
[non-exhaustive](../../cellgraph/README.md#the-crossing-verdict), so nothing
outside `cellgraph` can build one — so every copy is one the graph priced.
`cross_here` also reports whether its operand pinned, since a pinned value is
the carrier's own storage. `cross_views` crosses several operands at once, as a
hop's successor wakes the values handed to it one by one: it links the first
pinned operand's home branch where the destination has laid none and merges
the rest, so each merge stops at what that link reaches, and leaves uncopied a
copied operand its caller will place elsewhere.

**A part keeps its storage.** A value handed on from where it rests is read
through doors that never write: `Value::along` reads the stored part a record
field label reaches, through every tagged payload first, at the part's own
memo, and `stored_fields` a plain record's runs as stored, whatever type the
record is seen at. `Value::captured` reads the value a function's capture slot
holds, as an activation of it reads that capture: a value word as itself, an
edge as the fellow member of the knot it names. Those are two of a kept
path's steps (`PathSegment`): a label and a capture slot; the third, a
module's member slot, is the knot's to read, since a module is opaque here.
`same_storage` tells whether two values are one storage —
the same scalar, restamps of one run, or headers over one knot member —
without walking either, and `restamped_at` re-lays a value at the type a
restamp of the same storage carried, where the value lands.

A crossing door requires its family to be
[`Covariant`](../../cellgraph/src/reattach.rs), which a generic `ValueFamily<XF>`
cannot show, so `cross` and `cross_here` state it as a where-clause and each
concrete family — `ValueFamily<NoKnot>` here, `KValueFamily` in
[knot](../knot/README.md) — says `covariant!` once.

The graph consults an embedder closure for each operand's verdict, and this
module owns it: [`verdict`](crossing.rs) copies when the copy costs less than a
`COPY_RATIO`th of the bytes a pin would newly retain, and pins otherwise. The
comparison saturates, and the occupancy the prices also carry does not move it.
The [scheduler](../scheduler/README.md) builds its graph with this closure, so
every crossing a running program makes is priced by it; how a value reaches
another cell at all is [its delivery](../scheduler/README.md#delivery). Both
doors are generic in the graph's delivery bundle, so a step at any bundle —
`NoDelivery` in a fixture, `KDelivery` under the drain — reaches them.

## Views

Data is immutable, so an edit builds a new value from slices of its source and
the parts spliced between them. A slice, a concatenation or a splice of a list
or a string is a **view**: it reads through its sources' runs rather than
laying down cells or bytes of its own, and a view built from views is one view,
not a chain.

**A view is invisible.** A slice carries its source's type, and a concatenation
or a splice the type a flat list of its elements carries, derived from its
sources' element types alone, whether it is a view or written flat. Equality, rendering and `satisfies` read through a view, so a program
tells it from the flat value only by what it costs.

**A crossing resolves a view.** A copy writes only the elements a view shows,
as flat runs at the destination, and the copy holds nothing of its sources. A
view weighs what that copy writes, so the [verdict](#crossing) copies a small
view of a large source rather than pinning the source whole; a pinned view
stays a view.

**Only a structural transformation is a view.** A view remaps its sources'
indices and runs no koan code. A transformation that runs code, as `map` and
`filter` do, resolved at a crossing would run that code inside a copy, which
could fail, never finish, or perform an effect wherever the value happens to
cross, at a cost no weight measures. Such a transformation is lazy only as a
stream, a value of its own type, and a copy of a stream copies its pending
call without forcing it.

## Dict key order

A dict is two aligned runs in the region — keys sorted, cells beside them — so a
lookup is a binary search and **entry order is key order**. A key is a string, a
number or a bool behind a private representation, and every door that makes
one refuses NaN and folds `-0` to `0`, so the key order agrees with IEEE
equality on every key there is. The order is total: every bool, then every
number in numeric order, then every string by bytes. Any other value — a
[sealed](#what-a-value-is) scalar included — is refused, naming its own type. A
repeated key keeps its last occurrence. The sort and the de-duplication are
staged in a `BumpVec` over the caller's scratch, and string keys are written
into the dict's own region wherever they borrowed from.

A dict keyed this way is a value, not a table: it is built once and never
written. A cell-resident keyed table that a binding channel writes into is the
scope layer's shape, per [memory](../memory/README.md#shapes-not-instantiations).

A record is laid out the same way over field symbols, so a field read is a
binary search too, and two records compare order-blind. Rendering is the one
place symbol order would show, so a record renders its fields in the order of
their names' text.

## Content digests

A value's **content digest** ([digest.rs](digest.rs)) identifies it for the
life of the loaded program by what it holds, never by where it sits: two values
of equal content digest alike wherever they were built. It is what a
[carrier](../knot/module/README.md#the-view-door) is keyed on, per the
[module design](../../design/modules.md#a-module-is-its-content). A digest is
the low 128 bits of a BLAKE3 hash over one recipe: a domain tag, the value's
own scalar payload, then its parts' digests.

A plain value digests as what [the door](#views) shows of it at the type it is
seen at, exactly as the [deep copy](#crossing) lays it down, so a copy digests
as its source and a cell its type hides counts for nothing:

- A scalar digests from its payload: a number from its bits, a bool, `null`, a
  string length-prefixed, a type value from its handle and the kind it carries.
- A list, dict, record or tagged value digests its kind, its seen type's handle
  and the contents the door shows, each part at its type there — a list its elements in order, a
  dict each key and cell in key order, a record each shown field's name and
  cell in symbol order, so field order is blind, and a tagged value its payload
  at its representation. The seen type is part of the recipe, so a retype
  changes the digest even where it hides nothing.
- A [knot member](../knot/README.md#what-a-knot-member-is) seen at its own memo
  digests as its knot beside its index there: the knot over each node's content
  in index order, a data node read through the door at its memo and an edge to
  a sibling hashed as its index, so values that reach one another — closures
  that call one another, a ring of containers — digest as the one knot they are
  tied into. The layer above says what a node's content is: a member
  [lists](../knot/README.md#content) the values its knot holds and hashes the
  knot over their digests.
- A knot member seen at any other type — under a header, or a data node read
  at a part's type — digests as that type over its bare member's digest
  (`retyped_content`). A member's content is its knot's, never the members a
  type shows, so a signature that hides the capture branding a module keeps
  the brand in its digest.

Inside a knot each node is digested at its own memo, so a cell a tagged node's
representation hides still counts there, as a copy of the knot keeps it. That
over-distinction is sound: keying two equal things apart costs only a key.

**A digest is computed on demand**, and kept where the load says the demand
recurs: a view asks for its source's, a call of a keyed body for the argument
at each keyed parameter ([keys](../build/README.md#keys-which-bindings-make-a-settlement)),
and nothing else, so a value no view or key reaches never pays for one. A
value carries no digest but a module, which keeps its content from the first
view or key that reaches it; a slot the load marks as digested — the source of
a capture of a module that a `:|` in the slot's activation, or a block inside
it, views — keeps its
value's digest beside it, filled by that view or handed in at the frame's
birth by a call passing a digested slot bare
([a module is its content](../../design/modules.md#a-module-is-its-content)).
A demand walks what the
value reaches through one `Digests` memo over the demand's scratch, keyed by
the resident each part sits in and the type it is seen at — one node seen at
two types shows two surfaces — and by its root for a knot, so a part shared
many times over, and a knot met through any of its members, is digested once
per demand. The walk runs over an explicit stack, as the copy does: a composite
is a frame over the parts its surface shows and a knot member a frame over the
values its knot holds, so neither a value's depth nor a chain of knots grows the
call stack.

A literal inside a body's [code digest](../scope/README.md#resolution) is
hashed as syntax, not as the value it lowers to.

**Every recipe is a function over digests.** `literal_digest`, `type_digest`,
`knot_digest`, `member_digest`, `function_content`, `body_content`,
`view_content`, `retyped_content` and `edge_at` each hash one recipe from its parts' digests, and
the run's walk calls them. The load calls the same functions over a
[content tree](../scope/README.md#load-time-types), so a closed tree digests
as the value its expression evaluates to, and no recipe is written twice.

## Equality and rendering

`Value::equals` is what `==` means over data. Numbers follow IEEE; a tagged
value compares its identity before its payload, so it never equals its bare
payload, a sealed one included;
two type values are equal when they name the same handle; two quotes' code
compares as syntax, part by part with spans ignored and marks included, then
the values its `$` names and its supplied holes bind, name by name, under the
pair set circular values use below
([quotes](../scope/README.md#equality-and-knots)). Each side compares at the
type it is seen at, through [the door](#the-type-memo-and-satisfies): a record
by the fields that type names, a tagged value's payload at its representation.
Containers compare their contents **only when their seen types are related**,
one satisfied by the other in either direction — an empty list of strings and an empty list of
numbers are unequal. That makes `==` intransitive across ascriptions by design.

**A module has no structural equality.** `equals` answers
`Result<bool, Incomparable>`: a comparison with a module or a barrier on either
side is `Incomparable`, which the `==` builtin reports as an error rather than
`false`, and so is a pair of related containers whose aligned cells reach one.
A function compares by its identity and its instance solution, then its
closure bindings under the same pair set ([knots](../knot/README.md#equality-and-rendering)). A
knot member under a header compares as its bare member seen at the header's
type, so a function passed through a function-typed slot equals the function
it was.
Every aligned pair is compared, so an unequal pair before an opaque member does
not hide it. A container pair with unrelated types is still unequal without
descending, whatever it holds.

**Circular values compare as a bisimulation.** A data node compares as the
plain value of its kind would, its links resolved through it, so a node and a
plain value of the same kind and contents are equal. Two nodes compare under a
set of pairs already entered, each member beside the type it is seen at, since
one node reached at two seen types shows two surfaces: a pair is recorded before its
cells are compared, and a pair met again while recorded counts as equal. That
is the coinductive hypothesis — the greatest fixpoint, bisimilarity — and it is
sound because every result is a conjunction: a hypothesis never turns an
unequal pair equal, and every recorded pair is fully compared by the call that
recorded it. So a one-node ring and a two-node ring with the same contents are
equal. A node against a plain value records nothing, since the plain side is
finite and bounds the descent. A seen type always lies above the memo and is
built from the types the program declares, so a cycle returns to a pair already
entered. Plain values and data nodes share one reading, the door's `Surface`, so
equality, rendering, the mark pass below and the deep copy are written once over
both.

**Every walk here runs over an explicit stack** in the scratch it is handed, so
none grows the call stack with a value's depth. Equality pops pending pairs: a
pair is a gate — related types, keys, names, lengths, identity — or a leaf, and
a gate that passes pushes its children's pairs. Every answer is a conjunction
and any incomparable pair decides, so neither the answer nor `Incomparable`
depends on the order pairs are visited in. A quote's syntax is the exception: it
compares recursively, since parsed syntax nests no deeper than the parser's
[depth limit](../parse/README.md#the-syntax-depth-limit).

`Value::render` is the surface `PRINT` writes: a string bare, a dict key quoted
so `{"1": x}` and `{1: x}` read apart, `[a, b]`, `{k: v}` in key order,
`{x = 1}` in field-name order, a tagged value as its seen type's name around its
payload, a type as its name, a quote as its body's surface with each mark as
written and never what a name binds, a function, a module or a barrier as the
name of the type it is seen at, a header's included, which is a module's signature — its closure bindings or members
are program state and never print — and a data node as the plain value of its
kind.

**A cycle prints with labels.** A mark pass walks depth first from the first
data node the write meets that no earlier pass entered, entering each node
once, and records every node reached again while it is still being entered;
every cycle holds such a back edge, so every cycle holds a recorded target, and
a plain value with no data node is walked once. A node is entered, marked and
labelled beside the type it is seen at, so a narrow view of a node that holds no
cycle never hides the cycle a wide view of it closes. The write labels a target at
its first occurrence, `@0 = …`, and writes every later occurrence as `@0`, so
it stops wherever a cycle closes: `LET a = (Ring {next = a})` prints
`@0 = Ring({next = @0})`. Labels count from zero per render in order of first
appearance, and a node that is no target prints inline each time it is reached.
The mark pass keeps a frame per composite it is inside, and the write keeps a
stack of pieces still to write: a composite writes its opener and pushes its
children, separators and closer in reverse.

`Value::lower_part` builds a value straight from a region-pure AST part — a
scalar or string literal, or a container literal whose every element lowers and
whose every dict key is a scalar literal — and refuses anything that needs
dispatch or a scope, a quote included, since its code binds its `$` names where
it is written ([the quote door](../knot/README.md#a-quote)). The part is checked whole before anything is
written, so a refusal leaves the region untouched.

## Working expressions

A [`KExpression`](../parse/README.md#the-ast-borrowed-copy-and-splice-free) is
parsed AST and never changes. A [`WorkingExpression`](working.rs) is a
dispatch's own copy of one, built in the executing cell's region, whose slots
the scheduler rewrites. Each slot is a [`WorkingPart`](working.rs):

- `Ast` — a part of the node it was made from, a pointer copy at `'graph`;
- `Spliced` — a resolved sub-result, a `Value` reachable at the cell, beside the
  bare name the slot held before the splice so a diagnostic quotes the operand
  as the source spelled it;
- `Expression` — a nested node the scheduler synthesized, as an operator-chain
  fold's accumulator is;
- `RecordType` — a `:{…}` body whose co-declared references are threaded,
  kept apart so the slot still classifies as a record type;
- `StagedSlot` — a positional hole whose value a sibling dispatch is producing.

A working expression carries the same [`NodeCache`](../parse/ast/shape.rs) a
parsed node does. `from_ast` carries the parsed node's cache over whole — at
`'cell`, which the `'graph` borrow shortens to — since a splice substitutes
slots one for one and writes no keyword. `respliced` keeps the key and reads
only the head class again. A node the scheduler builds from scratch computes
its cache from its own key and has no binder plan, because a binder is always
parsed AST. Every working expression carries a `SourceRef`: a working copy the
AST node's own, and a synthesized node its origin's file and the extent its own
parts span, or the origin's extent when none of them is spanned.

A working expression is never a value and never crosses a cell: a continuation
captures it at the cell's own lifetime. That is what keeps the AST splice-free —
a resolved sub-result or a staging hole exists only here.

## The import rule

**Outside doc comments and `#[cfg(test)]`, `values` names `crate::memory`,
`crate::parse`, `crate::source` and `crate::type_lattice`, and nothing else in
the crate.** It names no scheduler type, no scope type and no function type,
so its knot-member arm is a parameter rather than a function or module arm. The
compiler cannot enforce a module boundary inside one
crate, so [`tests::boundary`](tests/boundary.rs) reads this module's own source
and fails on any other `crate::` path, on an owning heap type (`Rc`, `RefCell`,
`Box`, `Vec`, `String`) outside the test files, and on a lifetime spelled with
a name the rest of the stack retired.

## Testing

The unit suite ([tests.rs](tests.rs)) runs every door, crossing and relation
over a fixture that owns program storage, a root builder the test's load-time
work interns into, frozen into the program's one load when the first cell's
chain is built beside an empty agent tier, and a cell graph over that storage
to run steps in; each cell's chain reads the load and the agent tier, then the
cell's own branch, so a type a value door
builds in a cell lands in that cell's branch. A test finishes its load-time
interning before its first cell, as a program's load ends before its run
starts. Circular values are exercised
without `knot`: the fixture closes the parameter with a test-only member
whose every node is a data node, tied through `KnotPlan` with memos supplied
by hand, and the suites cover the door over each kind, a payload seen at its
representation, a retype as a seen type restamped and a data node restamped
under one header over its bare member ([tests/surface.rs](tests/surface.rs)), the `linked` doors, the construction
rule and the seal ([tests/construction.rs](tests/construction.rs)), bisimilar and unequal rings and a header
compared at its type ([tests/equality.rs](tests/equality.rs)), labelled and
shared-inline renders and a header over a cyclic node labelled at the top
([tests/render.rs](tests/render.rs)), a ring crossed under a copy and a pin,
a data node seen at another type copied under a header,
two members of one ring crossing as one copy of it, and a copy laying down only
what each part's seen type shows ([tests/crossing.rs](tests/crossing.rs)), a
result merging its home's branch, a copy adopting each laid part's memo and
none hidden, a copied type value read after its home is released, and the
habitat check refusing a value no crossing carried ([tests/habitat.rs](tests/habitat.rs)), `satisfies` by a node's memo
([tests/satisfaction.rs](tests/satisfaction.rs)), the fill's laws — order
independence, that it only narrows, idempotence, the fill of two fills and an
invisible memo — beside its cases, its depth and its cost
([tests/fill.rs](tests/fill.rs)), the hides bit set by a restamp and cleared
by a copy, a part's own type narrowed by its view, and a crossing copy keeping
each part's own type where its holder fills it ([tests/hiding.rs](tests/hiding.rs)), a cell read at its filled type
over its own storage ([tests/surface.rs](tests/surface.rs)), and a chain of newtypes a
hundred thousand deep rendered, compared, crossed and digested on the default
test thread, and a chain of headers as deep digested and crossed
([tests/depth.rs](tests/depth.rs)), and the memo bits — masked out of a
weight's bytes, set by the doors, a restamp, a copy and an exit, cleared by
a porous part or chain, and an exit of a value that meets them laying nothing
([tests/settled.rs](tests/settled.rs)). Three tests join the koan
[Miri slate](../../observe/miri_slate.md), the paths only `values` drives:
`a_copied_list_outlives_its_home` — a deep copy laying down strings, a list and
a dict whose string keys are written inside its key run's `fill`, read after
the region it was copied from is released —
`a_copied_type_value_names_a_type_its_destination_resolves`, a type node
re-laid in the destination's branch and read after its home is released, and
`a_settled_bit_written_through_a_pinned_header_reads_in_a_later_step`, a
header pinned from another cell's region marked settled through its `Cell` and
read in a later step. The plain pinned and kept paths are `cellgraph`'s own
slate.

The digest recipe is [tests/digest.rs](tests/digest.rs): equal content alike
across cells, key order and field-order blindness, a retype changing the digest
while a hidden cell counts for nothing, each of the copy's four narrowing paths
— and a list whose fill gives a record a field it does not store — digesting as
its source, and a shared part digested once per demand; a chain of
knots as deep as the chain of newtypes digests beside it.

## Open work

- [Slicing and splicing](../../roadmap/metaprogramming/slicing-and-splicing.md) — views
  over lists and strings, resolved at a crossing.
- [Yielding iterators](../../roadmap/rewrite/yielding-iterators.md) — streams,
  the lazy transformations that run koan code.
- [Recursion over run-time types](../../roadmap/rewrite/recursion-over-run-time-types.md)
  — the lattice walks over a value's carried type, as deep as the value.
- [Gradual typing's unplanned work](../../roadmap/gradual-typing/README.md#unplanned-work)
  — whether `Module` and `Code` re-stamp a value.
- [Unplanned work](../../roadmap/rewrite/README.md#unplanned-work) — a value's
  weight counting a knot once per reference, and a copied knot keeping a tagged
  payload's hidden fields.
- [A digest memo on every value](../../roadmap/gradual-build/value-digest-memos.md)
  — a value's digest kept on the value, so a chain a loop views a new module
  over each hop is walked once.
