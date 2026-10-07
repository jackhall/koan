# The node vocabulary

Part of [the type lattice](README.md)'s design.

A [`TypeNode`](node.rs) is one interned type's content: a variant tag, a scalar
payload, and **handles to its child types** — never owned substructure. Every run
a node holds is a slice in the run region, so a node is `Copy` and carries no
drop glue, and reading one out of the registry copies a few words rather than a
subtree.

- **Leaves** — `Number`, `Str`, `Bool`, `Null`, the two bounds `Any` (the top,
  and the default bound of a rigid variable) and `Never` (the uninhabited bottom,
  the identity element of both `join` and union canonicalization), and two of the
  three family tops, `AnyValue` (spelled `Value`) and `AnyCode` (spelled `Code`).
  See [three families](#three-families).
- **Code kinds** — what a piece of written code is: `Block`, `Expression`,
  `Declaration`, `Binder`, `Literal`, `Symbol`, `Name`, `Keyword`, and the four
  unspellable kinds `Identifier` (a value name), `TypeNameToken` (a type name),
  `SigiledTypeExpr` (a lone `:(…)`) and `RecordType` (a lone `:{…}`). A quote
  needing no name is typed by one, and a builtin slot read as written is typed
  by one or by a container of them. They are
  ordered among themselves by the code tree ([the code family](#the-code-family)) and all
  lie under `Code`.
- **`CodeNeeding`** — a code kind and the names its code needs where it is
  built, spelled `:(Expression NEEDING #[y])`: a quote's carried type, and a
  code parameter's. Its names are a symbol-sorted set and never empty, since a
  kind needing nothing is the bare kind.
- **`OfKind(KKind)`** — a type-accepting argument slot carrying the shallow
  [`KKind`](kind.rs) it admits. It is **type-channel only**: it admits a type
  *value*, never a runtime instance. A value is matched by a type, never by a
  kind.
- **Composites** — `List`, `Dict`, `Record` (structural, width- and
  depth-subtyped), `KFunction` (a named-parameter record plus a return, over a quantifier
  group of its own where a `FOR ALL` declared one),
  `Union` (canonical: deduplicated, no nesting, no member below the rest, two or
  more, in the order first written), and `ConstructorApply`.
- **`ExpressionShape`** — the type of a keyworded, positional definition reached
  by dispatch: the interleaved keyword/argument element sequence a call must
  spell, each slot's [priority class](solving.md#priority-classes), the type parameters
  bound ahead of it, and the return. It is
  *representationally* distinct from `KFunction`: a lambda takes a record of named
  arguments and is reached by name; a shape is reached by its keyword sequence and
  its argument *positions* are load-bearing, which a canonically ordered record
  erases. So no shape is ever equal to, satisfies, or is satisfied by a lambda
  type. What the two *share* is the binder: both carry a quantifier group, and
  every walk asks [`binds_quantifiers`](node.rs) rather than naming either arm.
  Only a non-empty group binds: a shape or a function type with none binds
  nothing, so a `Quantified` inside one reads the enclosing group, and one
  holding no variable is concrete.
- **Three rigid variables**, and the split is deliberate. `Quantified` is
  positional and bound by the enclosing binder, so two binders alpha-equivalent
  under a renaming intern to one node, and a free one in a declared slot is what
  a call solves. `Parameter` is *named*: a signature's head parameter, which its
  members read and `WITH` pins by name, or — carrying a `ContentKey` — the
  carrier an opaque view hides one behind, keyed on the content the view hides
  ([module design](../../../design/modules.md#carriers-and-paths)). A **lexical variable**
  (`Lexical`) is a name only a run binds, read where the program loads
  ([the type channel at load](../../../src/elaborate/README.md#the-type-channel-at-load)):
  it is positional by its level along the lexical chain that declares it, and
  carries its name, which renders it. No binder captures it and no solve binds
  it. The three share the rigid rule in
  *fits*, the substitution mechanism, and the role of the rigid side in a
  specificity check. Each is **bounded by** a closed type — a `KType` that holds
  no opaque carrier and is not `Never` — and lies under its bound and
  under everything above it, a union included: a variable bounded by
  `Number | Str` lies under `Number | Str | Bool`, though under neither member.
  A carrier records the bound its view's source met, but as a variable it is
  bounded by `Any`: outside its view it reveals no bound, and only a
  signature's fit reads the recorded one
  ([*fits* over signatures](relations.md#signature-types)).
  A lexical variable has a **lower end** too, a closed type under its bound:
  below it lie itself and whatever lies under that end, where below
  `Quantified` and `Parameter` lie only themselves and `Never`. A name a
  `FOR ALL` declares has the lower end `Never`; a later priority class reads an
  earlier one's variable as a lexical variable between two ends
  ([priority classes](solving.md#priority-classes)).
- **Signature types** — three nodes, each read as a set of
  [applications](relations.md#signature-types). A `Signature` is owned interface content: a
  `SIG`-declared interface, a module's self-sig, a view's signature and the
  empty signature that `:Module` lowers to are all this one node, distinguished
  only by the schema. It carries no binder and no label: two textually identical
  declarations are one type. A `SignatureApply` is a declared signature with
  some of its head parameters pinned, `Stack WITH {Elt = Number}`, and a
  `SignatureMeet` is two or more applications, none lying above another, sorted
  by handle so the set's identity is order-blind.
- **`Sibling` and `SetMember`** — the pre-seal and post-seal forms of a
  co-declared nominal group. See [recursive groups](identity.md#recursive-groups-identity-is-the-scc-not-the-declaration). A member's schema
  is a newtype's representation, or a type-constructor family's: the type a
  construction through it wraps, written over one `Quantified` per parameter —
  indexed in the symbol order the member stores its parameter names in — or
  none, for a family that constructs nothing. A sealed member is a leaf to
  interning's probes and to every rebuild, so those quantifiers never reach a
  type outside it. A
  `ConstructorApply` applies a family to a symbol-keyed argument record.

## `KKind` is the order on the type channel

Kinds form one subsumption lattice —
`AnyType > { Signature, ProperType > { NewType, TypeConstructor } }` — and
`OfKind(x) ≤ OfKind(y)` iff `y.admits(x)`. The signature wall lives here: a
proper-type slot names what can type an ordinary value, which a signature is not.
`AnyType` is a *slot* expectation only, never a classification `kind_of` produces.
As `OfKind(AnyType)`, spelled `Type`, it is also the type family's top.

## Three families

Below `Any` lie three disjoint family tops: `Value`, `Type` (`OfKind(AnyType)`,
the kind order's top) and `Code`. A type lies under a family top by its own node
shape, per the table in [`family_top`](order.rs):

| Family top | Node variants |
|---|---|
| `Value` | `Number`, `Str`, `Bool`, `Null`, `List`, `Dict`, `Record`, `KFunction`, `ExpressionShape`, `ConstructorApply`, the three signature types (a module is a value), `SetMember`, `Sibling` |
| `Code` | every code kind: `Block`, `Expression`, `Declaration`, `Binder`, `Literal`, `Symbol`, `Name`, `Keyword`, `Identifier`, `TypeNameToken`, `SigiledTypeExpr`, `RecordType` |
| `Type` | every `OfKind` |

The other nodes take their family from elsewhere. A union lies under a top when
every member does, and a rigid variable when its bound does. A variable bounded
by `Value` lies under `Value`, and one bounded by `Value | Type` under that
union. A variable bounded by `Any` — the default bound of a `FOR ALL` name and
of a signature's head parameter — lies under no family top: a value sealed behind
an unbounded parameter satisfies that parameter and `Any`, but not `Value`. A deferred return lies under
none, since its return is not known. So no type but `Never` lies under two tops.

A pair of tops is their union: `Value | Type` is the top of values and types
together, since a named node for it would be a second node for one type. A
union holding all three tops is canonicalized to `Any` by
[`union_of`](registry.rs), so the three families together are the whole
lattice. Left uncollapsed, that union would be a second top strictly below
`Any`, missing only the variables bounded by `Any`, which lie under no member.

## The code family

The code kinds form a tree under `Code`. A smaller syntax lies under a larger one
wherever it can stand in its place:

```text
Code
└─ Block                  statements; written, two or more
   └─ Expression          one statement
      ├─ Declaration      declares a name or a shape: VAL, a bodyless head
      │  └─ Binder        also installs where it is written: LET, an EXPR definition, …
      ├─ Literal          a lone scalar literal or nested quote
      ├─ Symbol           a lone token
      │  ├─ Name          a value or type name — what a declaration binds
      │  │  ├─ Identifier
      │  │  └─ TypeNameToken
      │  └─ Keyword
      ├─ SigiledTypeExpr  a lone :(…)
      └─ RecordType       a lone :{…}
```

A lone literal, name or keyword is an expression because dispatch evaluates it as
a statement in its own shape, and an expression is a block of one statement, so
a body slot typed `Block` takes `#(x)`. A block is no expression, since a slot
wanting one statement cannot take several. The tree is written once, as
[`KType::code_parent`](handle.rs) — the kind directly above a code kind — and
`within_code` walks it; both are `const` over handles, so the order's leaf arm
and a registry-free admission read the same edges. Join needs nothing of its
own: a union drops a member under another, so `Literal | Expression` is
`Expression`, and two kinds on different branches meet at `Never`.

**A code kind needing names** is `CodeNeeding`, built through
[`TypeRegistry::code_needing`](registry.rs), which sorts and deduplicates the
names and answers the bare kind for none. A bare kind is the kind needing
nothing, so one order covers both: a code type lies under a kind needing names
when its kind lies under that kind and every name it needs is among them. So
`:(Expression NEEDING #[y])` lies over `Expression`, over `Binder` needing `y`,
and under `Expression` needing `y` and `z`, and all of them lie under `Code`. A
parameter of that type therefore admits a quote whose needed names its list
covers. Two code types meet at their kinds' meet needing the names both need,
and at `Never` when the kinds do; the join is the ordinary union. A code kind
needing names holds no variable, so every walk treats it as ground.

Which kind a written quote is belongs to `parse`
([`KExpression::code_kind`](../../../src/parse/ast.rs)); the lattice holds only the order.
Containers of code are ordinary value types — `List(Name)` lies under
`List(Code)` and so under `Value` — since a container is a value whatever its
elements are.
