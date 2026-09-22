# Elaborate

Type expressions and type declarations turned into
[type lattice](../type_lattice/README.md) handles, read where they are written.
`elaborate` sits above [`scope`](../scope/README.md) and below
[`knot`](../knot/README.md): a function's type is elaborated from its
signature where the function is born, a component of type binders is declared
through [one door](#declarations), and nothing below `scope` can read a name.

## What a type expression is

A type expression is syntax in program storage — a bare type name, or a
parenthesized or sigiled group of parts. Its type names are not searched for:
the shape builder already resolved each one to a coordinate and recorded it as
a mention ([Resolution](../scope/README.md#resolution)), so
[`type_expression`](expression.rs) looks the mention up by the name part's site
through `BodyShape::mention` and reads it through the view of the activation the
expression is read in — `scope`'s `ActivationView`, the one read type at every
level, which names no habitat. The body runner reads a name only once its
binder's unit has run, so every read finds its slot bound. A name bound to a type value elaborates to that value's handle. A
parameter and return type of a callable are eager mentions of the enclosing
shape, so the activation a signature is read through is the one the callable
is born in.

Every composite is built from the handles its parts elaborate to, through the
registry's own doors:

- a bare name, `Number`, is the handle its binding holds;
- `LIST OF Elem` and `MAP Key -> Val` are the list and dict nodes;
- `Left | Right` is the canonical union of its two members, and a longer
  union arrives as `| [Left Right …]` — the
  [chained form](../scope/README.md#operator-groups) the shape builder
  already rewrote it into, so nothing here walks a union part by part;
- `:{x :Elem, …}` is the record type of its fields in written order;
- `FN :{x :Elem, …} -> Ret`, with or without `FOR ALL (names)`, is the function
  type over the schema's fields and the return;
- `EXPR (head) -> Ret`, with or without `FOR ALL (names)`, is the expression
  shape over the head's keywords and typed slots and the return;
- `Union.Tag` is the member of the union whose tag it names;
- `Pair {Key = Number}` is a declared type constructor applied to its arguments
  by the parameter names the family declares — every parameter named once and no
  name it does not declare — and `Number AS Wrap` is the same application spelled
  as arity-one sugar, reading the family's sole parameter name off the family.

**Quantifiers are positions, not mentions.** A name a `FOR ALL` group declares
is that group's quantifier at its written position, elaborated as
`quantified(index, Any)`, so two heads that differ only in what they call their
quantifiers intern to one shape. Groups nest: an `EXPR` type inside a signature
opens its own group, as does a `FN FOR ALL` type, and a name the innermost group
declares shadows the rest. A name only an *outer* group declares, read under a
nested group, is refused. A **bare** `FN` type opens no group at all, so a
parameter or return it spells inside a quantified head keeps reading that head's
variables.

## A callable's type

[`callable_type`](signature.rs) reads a callable's type off the builtin shape
node its body sits in — `BodyShape::form` of the body shape the binder births —
walking the node's parts by the [roles](../parse/builtin_shapes/role.rs) its
cached `BUILTIN_SHAPES` entry gives them:

- a `FN` is the function type over its `:{…}` schema and its return, quantified
  over its `FOR ALL` names;
- a **combined** form — `LET name = FN EXPR …`, with or without a group — is the
  function type over its head's slot names and its return, because a call
  through the `LET` name is by name; only its bucket registration carries the
  head's shape;
- a bodyless `EXPR` is the expression shape over its head and its return,
  quantified over its `FOR ALL` names;
- a binary `OP` is the shape `operand <symbol> operand`, returning its declared
  result, or its operand when it declares none, since a chain of it folds;
- a `UNARY OP` is the shape `<symbol> operands`, whose slot is a list of its
  operand, since its body's one parameter `operands` takes the whole run.

A function type binds its group in canonical form, which may renumber or drop a
variable, so `callable_type` hands back a **quantifier map** beside the handle:
each `FOR ALL` name the declaration wrote, paired with its index in the canonical
group, or `None` where canonical form dropped it. The knot stores the map on the
function node and a call reads each type parameter's solution through it. A
shape's map stays empty: its caller reads the group off the bucket instead.

**The name is the key, not the position.** A callee's type-parameter slots reach
its frame in the [type channel's](../scope/README.md#two-channels) own symbol order, not
the order the group was written, so nothing positional survives the trip; and the
interned type's `quantifiers` cannot stand in for the declaration's names,
because alpha-variants intern to one node and it carries whichever spelling
interned first.

A module body has no callable type here, and neither has a `USING` body: its
type is its signature, below.

## A module's self-signature

[`self_signature`](module.rs) reads a module's own type off the activation its
body ran in: **a value slot per value binder, at the type its value carries, and
a manifest member per type binder, at the handle it holds**. Nothing walks a
value — a slot's type is the memo the value already carries, which the tie
derived — and nothing reads the source, so a module's type is a fact about what
its body bound.

A module's signature therefore declares no abstract member: a body binds every
name it declares. Its **operator channel** carries the
[groups its body's shape holds](../scope/README.md#operator-groups) — a `GROUP`
body's own group, and nothing for a `MODULE` — so how a module's operators chain
is part of what it is, and a signature stating that chaining is one it satisfies.
Its keyworded channel is empty until
[dispatch](../../roadmap/rewrite/dispatch.md) gives a bodyless definition a
slot.

Every slot is bound: the caller runs the body to completion and only then
ties the binder ([the tie](../knot/README.md#the-tie)), so the self-signature
is a type and never a refusal. The handle is interned like any other, so two modules binding the same
members in either order are one handle.

## Declarations

A component of type binders comes into being through one door,
[`type_declarations`](declaration.rs) — the type channel's analogue of
[the tie](../knot/README.md#the-tie), and the other of the two ways a
component of binders becomes values. It takes the component and the activation
its members are declared in, and hands back one handle per member, in member
order:

- `NEWTYPE Distance = Number` — a newtype over its representation;
- `NEWTYPE (Key Val AS Pair)` — a constructor family: an empty-schema
  `TypeConstructor` member over its declared parameter names, the identity
  wrapper over its argument, so an application has a declared referent a koan
  program can write;
- `UNION Maybe = (Some :Number None :Null)` — the canonical union of one member
  per variant, the binder owning them all;
- `SIG HasLabel = (VAL label :Str)` — a signature;
- `LET Alias = Number` in the type channel — its right-hand side's type.

Which declaration a member is, and where its declared part sits, is read off the
node the shape recorded for it — [`BodyShape::declarations`](../scope/README.md#visibility),
by slot — and off that node's own
[builtin shape](../parse/README.md#the-builtin-shape-table-one-typed-entry-every-fact)
roles, so the table stays the one authority on what a form's parts are and a
`NEWTYPE (… AS …)`, which has no definition part, needs no case of its own.

**The caller binds.** The door takes no writer: it hands back `Copy` handles, and
minting each member's type value and naming the region it lives in are the layer
above's, exactly as they are for the tie.

### One window per component

Every member is read, and the whole member and binder list fixed, before any
schema elaborates — a schema naming a fellow must already have an index to name
it by. The component then opens one
[`RecursiveGroupWindow`](../type_lattice/README.md#recursive-groups-identity-is-the-scc-not-the-declaration):
one member per standalone declaration and one per union variant, each variant
owned by its `UNION` binder. A mention of a fellow elaborates to the relative
handle the still-open window minted — a member's own sibling, or a binder's
union of its variants' siblings — so `NEWTYPE Ring = :{next :Ring}` and a ring
of mutually recursive declarations seal with no placeholder, and identity is the
sealed SCC rather than the written group: two declarations of the same shape in
different programs are one handle. A projection off a fellow union is
`NoSuchMember` — until the group seals, the union declares no tag.

**Only a nominal member can close a cycle.** A transparent alias and a signature
name no fresh identity, so a cycle through one has no finite type: a cyclic
component holding a `LET` or a `SIG` member is refused at that member. A
non-nominal member is therefore only ever reached alone, and answers outside any
window. This is the type channel's restatement of the nominal cut
[the tie](../knot/README.md#the-tie) already makes for values.

### What a signature declares

A `SIG` body's statements are its members, read in source order by their own
builtin shapes' roles:

- `TYPE Carrier`, `TYPE (Held AS Boxed)` — an abstract member, bare or
  higher-kinded, and the only place a bare `TYPE` binds: outside a `SIG` it is
  refused;
- `LET Elem = Number` — a manifest member, fixed to its type;
- `VAL x :Elem` — a value slot;
- a bodyless `EXPR`, `OP` or `UNARY OP` head — a keyworded member, an operator
  head through the same builder [`callable_type`](signature.rs) reads a
  definition's operator shape through, so a head and the definition satisfying it
  cannot spell different shapes.

A signature's scope id is the sentinel, stamped here rather than round-tripped
through the declaring scope, which is what makes two textually identical `SIG`
declarations one type. A body's own names — its abstract and manifest members,
and a higher-kinded member's parameters — are declared by the definition and are
no mention of the enclosing shape, so the door resolves them against the members
it has already read. A member naming a *later* member of the same body is a
forward reference nothing has filled yet, and is refused.

A bodyless **`GROUP` head** is the signature's **operator channel**: one
[operator group](../scope/README.md#operator-groups) over the binary operator
heads its body states, under the mode its own form id names and, for a pairwise
head, the combiner it quotes. Each of those heads is a keyworded member like any
other, so a group declares both what its operators are and how a run of them
chains; two signatures differing only in that chaining are two types. An
operator head *outside* a `GROUP` declares its bucket alone and says nothing
about chaining.

A group head is refused when it would give a symbol a second chaining — a member
an earlier group of the same body holds, or a member of a builtin group the
declared group is not — and when its body is anything but binary operator heads,
when a member states a result of its own outside a pairwise group, or when it
names `==` or `!=`, which belong to no group. A binary head stating a result of
its own is admitted only where its symbol chains pairwise, read off the groups
this body declares, the builtin groups, and the frame the `SIG` is written in.

## Builtin shapes

A [`BUILTIN_SHAPES`](../parse/builtin_shapes.rs) entry states its bucket's
overloads as `static` data — a slot type per slot per overload, one return apiece
— because the parser probes that table before any registry exists.
[`builtin_shape_types`](builtin.rs) is the one door that turns such an entry into
[`ExpressionShape`](../type_lattice/README.md#the-node-vocabulary) handles: one
per overload, in overload order. A reserved bucket interns nothing — its slot
types exist to keep its parts raw so its miss stays a miss, not to name a callable
anything can reach.

The door is where a slot type stops being a recipe. A leaf slot already rests in
the table as its own `const` handle and passes straight through; the two compounds
a builtin slot uses — a union of leaves, the empty record — are interned here,
which is the whole reason the door exists, since no `const` computes a compound's
digest. Nothing here reads a name, so nothing here fails: an entry is not a type
expression, and a `NotAType` has no meaning over `static` data.

Every handle the door interns erases to the entry's own bucket key, so the typed
shape and the untyped bucket a node probes with cannot drift apart.

## Refusals

A type expression that does not elaborate is an [`Elaboration`](../elaborate.rs),
never a panic and never a guess:

- `NotAType` — a type name bound to something other than a type value;
- `NoSuchMember` — a union projection naming a tag the union does not declare;
- `Unsupported` — any other spelling: a `_` field, an outer quantifier read
  under a nested group, an application whose arguments are not exactly the
  parameters its constructor declares, and every declaration the door refuses —
  a cyclic component through a non-nominal member, a bare `TYPE` outside a
  `SIG`, a repeated union tag or family parameter, a union
  with no variant, and a forward reference inside a `SIG` body. An operator
  declaration or head naming `!=`, and an `==` whose result is not `Bool`, are
  `Unsupported` for the same reason a group head that would chain a symbol twice
  is: [`!=` is rewritten, never declared](../scope/README.md#operator-groups).

Elaboration writes nothing to a region: its transient runs live in the scratch
arena it is handed, and every node it builds is interned in the registry. A
refusal from the door binds no slot for the same reason: its window and staged
runs are scratch, and the registry it interns into is content-addressed, so a
handle minted before the refusal is content no name reaches.

## The import rule

**Outside doc comments and `#[cfg(test)]`, `elaborate` names `crate::memory`,
`crate::parse`, `crate::scope`, `crate::type_lattice` and `crate::values`, and
nothing else in the crate.** It reads each part's role off `parse`'s own
builtin shape table and the pair reader `scope` exposes to the crate, so a type
expression's parts are walked by the same facts the shape builder walked them
by.
[`tests::boundary`](tests/boundary.rs) reads this module's own source and fails
on any other `crate::` path, on an owning heap type outside the tests, and on a
retired lifetime name.

## Testing

[`tests/examples.rs`](tests/examples.rs) elaborates each production, each
refusal, and a callable's type off each builtin shape that births one, over a
program shaped and activated in a cell with every slot bound or left empty as
the test asks. [`tests/declarations.rs`](tests/declarations.rs) runs the declaration
door over the same harness: each form it elaborates, each group it seals — a
ring, a union and a newtype in one component, a ring written in either order
interning equal — the chaining record a bodyless `GROUP` head declares and the
two handles two directions make of one signature, and each refusal, asserting
the refused component left its binders' slots empty. [`tests/module.rs`](tests/module.rs) runs the self-signature
over a module body activated in a cell with its slots bound by hand: a slot per
value binder and a manifest member per type binder, the empty body as the empty
signature, two bodies binding the same members in either order interning equal,
a member carrying the type its value carries rather than one walked from its
contents, a `GROUP` body's self-signature carrying the group it declares and
and satisfying the signature stating it.
[`tests/builtin.rs`](tests/builtin.rs) holds the door's own laws: each
overload erases to the entry it came from, a bucket interns one handle per
overload and a reserved bucket none, and the one union a builtin slot names
interns as the union of its three members.

## Open work

- [Dispatch](../../roadmap/rewrite/dispatch.md) — the keyworded channel a
  bodyless `EXPR` or `OP` member fills, which a self-signature leaves empty.
- [Unplanned work](../../roadmap/rewrite/README.md#unplanned-work) — `WITH` over
  a signature, which the lattice specializes but no type expression elaborates.
