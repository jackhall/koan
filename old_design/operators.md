# Operators

[`OP`](../src/builtins/op_def.rs) declares a chainable operator in the enclosing
scope; [`GROUP`](../src/builtins/group_def.rs) bundles mutually chainable
operators under one reduction mode. Together they are the declaration surface that
populates the per-scope operator registry the
[chain reducer](expressions-and-parsing.md) walks — the reducer decides *how* a
recognized run reduces, and this surface decides *what* is a run and what each
operator does.

Both are ordinary builtins: they add no dispatch-classifier case and no reserved
lead keyword. What buys that is the quote.

## The symbol is quoted

An operator symbol arrives as a `#(...)` quote — a parse-static
`QuotedExpression` part, captured by the parser as
data. A quote is a *slot* for dispatch purposes, so every `OP` / `GROUP` overload
keeps a **fixed** untyped key and matches whatever symbol it is handed:

```
OP #(+) OVER Number = (left + right)
```

An unquoted `+` would be a `Keyword` part, which lands in the expression's own
untyped key — so every operator would key a different bucket and no fixed overload
could match. The quote is what makes the symbol an argument rather than syntax.
The same rule governs a pairwise group's combiner (`PAIRWISE FOLD #(BOTH) LEFT`).

The declaration surface's own spelling is reserved: `OP`, `UNARY`, `OVER`,
`GROUP`, `FOLD`, `PAIRWISE`, `LEFT`, `RIGHT`, `=`, `->`, `:|`, `:!` cannot name an
operator. Every other keyword-classified token can, including an all-caps
alphabetic name (`OP #(MAX) OVER Number` is fine).

## Binary operators

```
OP #(<sym>) OVER <Operand> = (<body>)
OP #(<sym>) OVER :<Operand> = (<body>)          -- the `:` is optional after OVER
OP #(<sym>) OVER :(LIST OF Elt) = (<body>)      -- or a sigiled type expression
OP #(<sym>) OVER (LIST OF Elt) = (<body>)       -- the sigil is optional here too
```

The last two are the same declaration: `OVER` and `->` are binder-form **type slots**, where
a bare `(…)` is admitted as the sigiled spelling at parse
([type-language-via-dispatch.md § The bare parenthesized spelling in a type slot](typing/type-language-via-dispatch.md#the-bare-parenthesized-spelling-in-a-type-slot)).

The body binds `left` and `right`, both of the operand type, and its result type
*is* the operand type — a fold member feeds its result back in as the next
operand, so the two coincide. The body captures its declaring scope, so it sees
its sibling module bindings exactly as a bare definition body does, and the declaration
evaluates to the function it declares.

A member of a **pairwise** group has a result type of its own, and says so:

```
OP #(<) OVER Number -> Bool = (…)
```

Such a heterogeneous form is admissible *only* inside a `PAIRWISE` group, where a
combiner folds the pair results. Declared anywhere else it errors: without a
combiner its own run would left-fold ill-typedly (`(a < b) < c`).

## Unary operators

```
UNARY OP #(<sym>) OVER <Operand> -> <Result> = (<body>)
```

A unary operator takes the **whole run as one list**: the body binds `operands`
of type `:(LIST OF Operand)`, and infix (`a ~ b ~ c`) and prefix (`~ [a b c]`)
forms reduce to the same keyword-first call. The `-> Result` segment is mandatory
— the body consumes a list of operands, so there is nothing to default the result
type from. The result-less shape therefore has no success reading at all: all three
of its spellings (`UNARY OP … OVER <Operand> = (…)`, the `LET`-combined twin, and the
bodyless head a SIG body would declare it with) are **reserved** keys, registered by
nothing and refused to user registration at the
overload write door, so they always reach the
[dispatch-miss diagnosis table](../src/machine/model/miss_diagnostics.rs)'s pointed
"must declare its result type" message. A two-operand use (`a ~ b`) names one keyword and so dispatches as a
plain keyworded call rather than a chain; the declaration registers a synthesized
binary **bridge** whose body is `sym [left right]`, so that surface lands on the
one list body the user wrote.

A unary operator is therefore always the same **triple**: the list-form overload
under the key a prefix use or a reduced run computes, the binary-form overload
under the key a two-operand use computes, and a size-1 `Unary` registry entry
under the symbol. One door writes it
([`register_unary_operator`](../src/builtins/op_def.rs)), deriving each key from
the signature it is handed rather than spelling it — an overload keyed any other
way would sit in a bucket no koan expression ever computes, and the operator
would silently never dispatch. The caller supplies the two bodies: a `UNARY OP`
synthesizes koan-AST bodies (the user's list body plus the bridge), while the
builtin type-union `|` — itself a unary operator — supplies native ones that
compose a [`KType`](../src/machine/model/types/ktype.rs) directly.

Because it chains with nothing, a unary operator can be no group's member.

## Operators as values: the combined statement form

An operator declaration evaluates to the function it declares, and prefixing
`LET <name> =` binds that function under a name in the same statement:

```
LET plus = OP #(⊕) OVER Number = (<body>)
LET before = OP #(≺) OVER Number -> Bool = (<body>)
LET collect = UNARY OP #(~) OVER Number -> :(LIST OF Number) = (<body>)
```

Every declaration surface above has this twin: both `OP` arities and the
`UNARY OP` form, over the same operand/result type carriers, built from one
element list per surface so the two spellings cannot drift
([op_def.rs](../src/builtins/op_def.rs)). The statement is one binder filling
both install channels — the value name and the bucket key(s) the declaration
registers under, two of them for `UNARY OP` — so the operator reduces its runs
*and* the bound name reaches the same `KFunction` as a first-class value. For a
unary operator the bound value is the list body, the operator's primary function.
Binding is a statement-level act, so this is the only spelling that reaches both:
an `OP` in a plain `LET`'s value slot is a `NestedBinder` error
([name-placeholders.md § the position rule](execution/name-placeholders.md#submission-time-binder-install-and-the-position-rule)).

A group's member scan reads both spellings, so a combined declaration inside a
`GROUP` body is an ordinary member.

## Groups

A `GROUP` **is** a module: it binds a module value under a snake_case name, its
body is an ordinary module body, and `USING <group> SCOPE (…)` opens it. What it
adds is one shared registry record, so *distinct* members mix in a single run.

```
GROUP vec_ops FOLD LEFT = (
  (OP #(+) OVER :(LIST OF Number) = (…))
  (OP #(-) OVER :(LIST OF Number) = (…)))

GROUP num_compare PAIRWISE FOLD #(BOTH) LEFT = (
  (OP #(BOTH) OVER Bool = (…))          -- the combiner, over the pair-result type
  (OP #(≺) OVER Number -> Bool = (…))
  (OP #(≼) OVER Number -> Bool = (…)))
```

`FOLD LEFT` / `FOLD RIGHT` give the group `FoldLeft` / `FoldRight`;
`PAIRWISE FOLD #(<combiner>) <LEFT|RIGHT>` gives it `Pairwise`, carrying the
combiner symbol and the direction the pair results fold in.

The members are read off the **unevaluated** body — a structural scan of its
top-level `OP` statements — and the full nonempty powerset of the member set is
registered into the group's child scope *before a single body statement runs*, all
subsets pointing at the one record. So declaration order inside the body does not
matter, and a mixed-member run reduces inside the group's own body as readily as
through a `USING` window. Only top-level `OP` statements are members: an `OP`
nested inside a definition or a branch declares an operator in *that* scope and joins
no group. Any other statement — a `LET`, a definition, the combiner's own `OP` — is
ordinary module content.

A group is the sole registrar for its members: a member `OP` writes the function
bucket only, while a *bare* `OP` (outside any group) also writes a size-1 registry
entry, so it self-chains — fold-left for a binary operator, unary for a
`UNARY OP`.

A Type-token group name registers nothing and takes the same respelling diagnostic
`MODULE`'s Type-token name takes, from the
[dispatch-miss diagnosis table](../src/machine/model/miss_diagnostics.rs): a group is a
module, and a module is a value.

## The registry record lives in the declaring scope's region

An [`OperatorGroup`](../src/machine/model/operators.rs) is koan semantic data, so
it lives where all of it lives: the declaring scope's region bump, through the one
allocation door `OperatorGroup::alloc`. The record is `Copy` and `Drop`-free — its
member set is a sorted, deduped slice of `KeywordSymbol`s, probed by binary
search, and a pairwise mode's combiner is one more symbol beside it — so
region death frees it with the chunks and nothing refcounts it.

That one allocation backs the whole powerset: each subset key in the scope's
`operators` table holds a sealed carrier over the *same* pointee
([`Bindings`](../src/machine/core/bindings.rs) stores a carrier plus a binding
index, the entry shape the `data` and `functions` tables take), so sharing is
address identity and installing `2^n - 1` keys allocates nothing but the
subsets' recorded renderings. The upsert that admits a re-declaration compares
those addresses first
and the mode-plus-member-set second. A `GROUP` body's own scope names the same
record as a plain reference at the scope's lifetime, which is what
`Scope::nearest_group_context` hands a member `OP` — the record is same-region
with the scope, so the answer cannot outlive the borrow it came from.

### One constructor mints both sides of the key

A registry key — the one a subset install writes and the one a live chain probes
with — is a **symbol-run digest**: the run's member `KeywordSymbol`s sorted by
symbol bits, deduped, and their digests hashed through
`KeywordSymbol::of_run`. Both sides mint through
that one constructor — the chain's probe from `operator_probe_for`
(shape.rs) as the parse freezes the node, the
powerset from `declared_run` at registration
([ops.rs](../src/machine/core/bindings/ops.rs)) — so a registered key and a real
chain's probe agree by construction rather than by two renderings matching, and no
probe path reads a glyph's text at all
([label-interning.md](label-interning.md)).

A singleton run's key is therefore *not* the bare member symbol: `{+}` as a
registered group and `+` as a token are distinct digests, so nothing can confuse a
group registration with a keyword. `declared_run` additionally records the members'
interned spellings, joined in the same sorted order, under the digest — that
recording is what an operator-conflict or cross-group diagnostic renders when it has
to name the probe.

A resolved group travels as a delivery envelope lifted at the scope that declared
it, so a chain reducing under an ancestor's group holds that ancestor's region for
as long as it reads the record. Nothing else holds it: once the declaring region
dies, the scope, its table, and the record die together, and the walk resolves
nothing rather than finding a record kept alive by a stray refcount.

## The pairwise combiner is an operator, invoked infix

A pairwise run dispatches each adjacent pair through its own member's body and
folds the pair results through the group's combiner. The record stores the
combiner's **symbol**, never a resolved function, and the reducer synthesizes the
infix shape `[left, Keyword(<combiner>), right]` at each use site. That shape
re-enters ordinary keyworded dispatch, so the combiner binds its two inputs
**positionally**, by signature shape — it imposes no parameter-naming convention
on the combiner, and it reaches a type-level combiner, which the function-value
lane could not.

In practice a combiner is therefore declared as an `OP` over the pair-result type
`r`, which makes the `(r, r) -> r` fold shape true by construction. Declaring it
inside the group body carries it through `USING` alongside the operator bodies.

Resolution is the ordinary scope walk at the chain's use site, so a combiner that
is missing, non-callable, or of the wrong arity surfaces as an ordinary error
*there*, not at declaration. Storing a symbol rather than a resolved function is
also what keeps [`OperatorGroup`](../src/machine/model/operators.rs) flat: the
combiner is one more bump-hosted keyword in the record's own region, not a
reference into whatever region the combiner's body happens to live in.

## Shadowing is type-gated, not forbidden

An operator declaration may name a symbol the builtins already use. The two
lookups it touches resolve differently, and between them that is exactly the
behavior wanted:

- The **function bucket** is builtin-first: dispatch consults the immutable
  run-global root before the scope walk, so the builtin `+` still wins for the
  operand types it declares (`Number`), and only other operand types fall through
  to a module's own body. `OP #(+) OVER :(LIST OF Number)` therefore adds list
  addition and leaves arithmetic alone.
- The **operator registry** is innermost-wins
  ([`Scope::resolve_operator_group_delivered`](../src/machine/core/scope/resolve.rs)): a
  registry hit carries a member set and a mode but no operand types, so it *cannot*
  type-gate. The builtin groups seeded into the root are found last — they are
  chaining defaults a declaring scope may override, which is what lets a scope
  declare `-` as `FOLD RIGHT` and have its own runs right-associate.

Within one scope, one operator has one chaining mode: two `OP` statements over the
same symbol and distinct operand types are two bucket overloads and one registry
entry (an idempotent upsert), while two that disagree on the mode are an error.

## Operators as signature members

A [signature](typing/modules.md) declares an operator with the definition's own
head, minus the `= (<body>)`:

```
(OP #(<sym>) OVER <Operand>)                            -- a fold member
(UNARY OP #(<sym>) OVER <Operand> -> <Result>)          -- the whole unary triple
(GROUP FOLD <LEFT|RIGHT> = (<heads>))                   -- a chaining group
(GROUP PAIRWISE FOLD #(<combiner>) <LEFT|RIGHT> = (<heads>))
(OP #(<sym>) OVER <Operand> -> <Result>)                -- inside a SIG PAIRWISE group only
```

The bodyless `GROUP` is the definition minus its **name**: a SIG binds no value, so
there is nothing to name, and the `=` plus the head list carry over unchanged. Its
members are read by the same structural scan of the unevaluated body a definition's
members are ([`group_def.rs`](../src/builtins/group_def.rs)), and a SIG group body
holds heads and nothing else — a statement the scan would skip is refused rather
than silently left out of the record.

Both halves of what a definition writes are declared. A head derives its bucket
key(s), parameter names and slot types from `operator_shape`
([`op_def.rs`](../src/builtins/op_def.rs)) — the one derivation the definition
registers through — so a head and the `OP` satisfying it cannot spell different
shapes, and the operand and result slots accept every type spelling the definition's
do (`Carrier`, `:(LIST OF Elt)`, `(LIST OF Elt)`). What each statement records:

| statement | keyworded channel | operator channel |
|-----------|-------------------|------------------|
| `OP #(s) OVER O` | `[Slot s Slot]` → `(left :O, right :O) -> O` | `{s}` → `FoldLeft` |
| `OP #(s) OVER O -> R` (in a SIG pairwise group) | `[Slot s Slot]` → `(left :O, right :O) -> R` | — the group's |
| `UNARY OP #(s) OVER O -> R` | `[s Slot]` → `(operands :(LIST OF O)) -> R` and `[Slot s Slot]` → `(left :O, right :O) -> R` | `{s}` → `Unary` |
| `GROUP <mode> = (<heads>)` | — its heads' | `{scanned members}` → the mode |

A head standing on its own declares the singleton record its surface implies, exactly
as a bare `OP` writes one; inside a group body the group is the sole registrar, the
same split the definitions take. One signature declares one chaining mode per
operator: a symbol named by two records is refused at the collector, as it is in a
scope's registry.

The heads and the bodyless group are meaningful only inside a SIG body. Outside one
each is refused naming the definition spelling, and an operator *definition* — any
arity, including the `LET`-combined form — inside a SIG body is refused naming the
head. The heterogeneous `OP #(<sym>) OVER <Operand> -> <Result>` head is admissible
exactly where the definition's heterogeneous form is, inside a `PAIRWISE` group, and
the result-less `UNARY OP` head is a **reserved** key carrying the same pointed "must
declare its result type" message the definition spellings carry.

### The operator channel is signature content

A schema's operator channel is a set of **chaining records** — a sorted member set
plus a mode — held in canonical order
([`sig_schema.rs`](../src/machine/model/types/sig_schema.rs)). It is content like
every other channel: it feeds the schema's content digest, so two signatures
differing only in how their operators chain are two types; it renders in a
signature's name (each member as its own head, then every record a member's head does
not already spell in full, as the `GROUP` head declaring it — `GROUP FOLD RIGHT {+ -}`);
it rides `TYPE OF`; it clones through a `WITH` pin, which names no operator; and it
intersects in a signature join, pairing same-mode records and keeping the intersection
of their members.

A record is spelled in full by its own member's head in exactly two cases: a bare `OP`
head declares a fold-left singleton, and a `UNARY OP` head a unary one. Every other
record renders — a wider one, and equally a *singleton at a mode no bare head implies*,
such as `(GROUP FOLD RIGHT = ((OP #(-) OVER Carrier)))`. Rendering that one matters:
it is a different interface from the bare `(OP #(-) OVER Carrier)`, and a name that
dropped the `GROUP` head would print the two identically — including in the mismatch
diagnostic that names the signature a module failed.

A member is rendered as an operator head — `OP #(+) OVER Carrier` — precisely when its
bucket key is an operator key *and* its symbol belongs to one of the schema's records.
Nothing finer distinguishes the overloads under such a key: a shape type carries no
argument names, so a member declared by a plain `EXPR` head at an operator key renders
as an operator head too once a record claims the symbol. A key no record names keeps the
plain head rendering, overload by overload. Both keys of a unary triple render as the one
`UNARY OP` head that declares them.

### Satisfaction: equal mode, member inclusion

A declared operator member is a keyworded member, satisfied by the same
most-specific overload selection every keyworded member takes
([modules.md § Keyworded members](typing/modules.md#keyworded-members)); the three
keyworded failures name it by its operator head.

Each declared **record** additionally needs a record in the module's own registry
whose member set *includes* the declared one — width, as in every other channel —
under an **equal** mode. At most one module record can cover a declared one, since
two records never share a member. Mode is matched exactly, not covariantly: a run
folded right and the same run folded left compute different things, so two modules
differing only in chaining mode are distinguished. The two failures name the members
and the modes — `no chaining mode covers ⊕` when the module supplies the buckets but
declares no group over them, and `operators ⊕ chain fold-left in the signature but
fold-right in the module` when it groups them another way.

### A declared pairwise group's combiner

A pairwise group's combiner is resolved by the ordinary scope walk at the run's use
site, so inside a `USING <view> SCOPE` window it must be one of the members the view
installs. A declared `PAIRWISE` group therefore names a combiner the signature itself
declares, checked at the SIG finish — after the whole body, so a combiner declared
below its group still counts.

**Where the combiner is declared is part of the interface.** Declared *inside* the
group body it is one of that record's members; declared beside it in the SIG body it
is its own singleton fold-left record. Both spellings pass the combiner check and
each is satisfied by the module that groups its combiner the same way, so a signature
mirrors the grouping it means to require — which, since declaring the combiner inside
the group body is what carries it through `USING`, is normally the inside spelling.

### A view installs both halves

An [ascription view](typing/modules.md#block-scoped-opening-using--scope) publishes
the declared members' overloads into its buckets — each coerced across the barrier
exactly as any keyworded member is — and **births one fresh record per declared
group**, over exactly the declared members, into its own registry. The record is born
in the view's region rather than adopted from the source: a record's whole content is
lifetime-free, so re-birth costs nothing and leaves the view holding no borrow into
the source.

Birthing over the *declared* members is what makes the registry half a narrowing like
the other two. A source group chaining `⊕`, `⊖` and `⊗` against a signature naming
`⊕` and `⊖` gives a window in which `a ⊕ b ⊖ c` reduces and `a ⊕ b ⊗ c` finds no
group at all.

The replay installs **without** the builtin-shadow guard. That guard exists to stop a
user definition from joining a builtin's bucket, and the source's own overload already
passed it where it was declared; a replay is not a second declaration. So a signature
may declare `OP #(+) OVER Elt` and a view over it ascribes, while shadowing stays
type-gated as everywhere else — inside the same window an arithmetic run still
resolves to the builtin.

### The EXPR-head spelling declares the bucket only

The bodyless `EXPR` head spelling of an operator key —
`(EXPR (left :Carrier + right :Carrier) -> Carrier)` — stays legal and means exactly
what it says: the bucket, and no claim about chaining. It declares no record, and is
satisfied by any module supplying the overload, grouped or not. A module supplying only
that half fails a signature that declares `OP #(+) OVER Carrier`, because the head
declares a `{+}` fold-left record the module has not.

## Visibility

An `OP` writes into its **enclosing scope** — a module body's child scope, a
`GROUP`'s child scope, a per-call function scope, or the top level — and a use site
finds it by the ordinary innermost-wins scope walk with lexical cutoff. The
`USING` window's borrowed façade surfaces a module's operator registrations
alongside its values and function overloads, so opening a group puts both its
member bodies and its chaining mode in scope
([modules.md § Block-scoped opening](typing/modules.md#block-scoped-opening-using--scope)).
An ascription view surfaces both halves too, narrowed to what its signature declares
([Operators as signature members](#operators-as-signature-members)): the declared
members' overloads in its buckets, and one record per declared group in its registry.

An operator declared *after* a run is invisible to it (lexical cutoff), while one
declared before it in the same submitted block resolves whatever order the
scheduler pops the statements in: the `OP` binder installs a pending-overload entry
under each bucket key its body will register, and a chain that misses the registry
parks on a visible pending declaration rather than erroring.

## Generic groups are functors

A group parameterized by a type is an ordinary
[functor](typing/functors.md) — a definition whose body is a `GROUP`:

```
LET make_ops = FN EXPR (MAKEOPS Elt :Type) -> Module = (
  GROUP result FOLD LEFT = (
    (OP #(+) OVER :(LIST OF Elt) = (…))
    (OP #(-) OVER :(LIST OF Elt) = (…))))
```

Instantiate it at a concrete type (`MAKEOPS Number`) when the member bodies need
no operation on the element type, or at a witness module satisfying a signature
(dictionary passing) when they do. Both are shipped definition mechanics; the group adds
nothing.

Selection is always **explicit**: an instantiation binds a module, and only a
`USING` window over that module surfaces its operators. No group is chosen for a
run by the run's operand type.

## Open work

- [Stage 4 — Property testing and axioms](../roadmap/old_predicate_typing/axioms-and-generators.md)
  — group validation: whether a combiner's `(r, r) -> r` shape, mode consistency,
  and algebraic laws (associativity, identity, inverse) are checked at declaration
  or trusted.
- [Stage 5 — Modular implicits](../roadmap/old_predicate_typing/modular-implicits.md)
  — implicit *selection* of a group by operand type, so a run finds its group
  without a `USING` window.
- [One kind-blind reader per shape slot](../roadmap/old_metaprogramming/one-reader-per-shape-slot.md)
  — [old_design/metaprogramming.md](metaprogramming.md) rules the quoted and
  unquoted parenthesized symbol one spelling; today the unquoted `OP (+)`
  registers but installs no park edges and errors inside a `GROUP` body.
- [Group members may arrive by splice](../roadmap/old_metaprogramming/group-members-by-splice.md)
  — late member join for an `OP` spliced into a group body by `EVAL`
  ([old_design/metaprogramming.md](metaprogramming.md)); today membership is
  decided solely by the pre-evaluation member scan.
