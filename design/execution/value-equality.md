# Value equality

`==` and `!=` compare two runtime values structurally and yield a `Bool`. The
engine is [`KObject::value_equal`](../../src/machine/model/values/kobject/equality.rs),
a per-variant walk; the operators themselves are the binary-only builtins in
[`src/builtins/equality.rs`](../../src/builtins/equality.rs).

## The operators

`==` and `!=` are ordinary builtins over `(left :Any) op (right :Any) -> Bool`.
Each `:Any` slot admits both value channels, so the body reads its operands as
raw `Held` cells: two objects compare by `value_equal`, two types compare by
digest ([`KType`](../typing/ktype/README.md)'s cross-lifetime `PartialEq`), and
a mixed object/type pair is unequal. `!=` negates a successful comparison and
propagates a banned-operand error unchanged.

They are **binary-only**: neither is seeded into any operator group, so equality
does not associate. A chain (`a == b == c`, or a mix such as `a == b < c`) draws
a keyword subset no group covers and surfaces a resolution error rather than
reducing pairwise.

## The structural walk

`value_equal` is cross-lifetime (`&KObject<'a>` vs `&KObject<'b>`): a spliced
expression part opens its delivery envelope at a fresh brand per side, so the two
carried values never share a lifetime. The whole comparison threads independent
slot and value lifetimes over the heterogeneous `KType` predicate suite. Values
are acyclic by construction (see
[Constructing circular values](../../roadmap/type_language/circular-value-construction.md)),
so the walk carries no cycle guard.

Per variant:

- **Numbers** follow IEEE: `NaN != NaN`, `-0.0 == 0.0`. (Dict *keys* are a
  separate, NaN-free domain — see [Key normalization](#key-normalization).)
- **Strings**, **booleans**, and **null** compare by value.
- **Lists** gate on comparability (below), then compare length and element cells
  pairwise. There is no pointer fast path: `[NaN]` is unequal to itself, matching
  element-wise IEEE semantics exactly.
- **Dicts** gate on both key and value type parameters, then compare by key
  lookup — for each entry in one map, the other must hold the same key with an
  equal value.
- **Records** gate on record subtyping (either direction), then compare
  order-blind: the same field-name set, each field's value equal. Field
  declaration order does not matter.
- **Tagged** values compare nominal identity first (`same_nominal` — set digest
  plus variant index); type arguments are a comparability gate (empty on either
  side is erased and therefore comparable, both populated must agree on arity and
  be pairwise related); then the payloads walk.
- **Wrapped** (newtype) values compare their nominal `type_id` via digest-based
  `KType` equality, then the inner payloads. A `Wrapped` value is never equal to
  its bare representation, and two distinct newtypes over the same representation
  are unequal — nominal identity, not shape, decides.
- **Expressions** compare by [structural syntax equality](#expression-equality).
- Every remaining cross-variant pair is unequal.

Cell-wise: two objects walk structurally, two types compare by digest, a mixed
object/type cell is unequal.

## The comparability gate

Containers (`List`/`Dict`/`Record`/`Tagged`) compare their contents only when
their memoized or ascribed type parameters are **related** — one `satisfied_by`
the other, in either direction. Unrelated parameters short-circuit the container
to unequal without descending. This buys two properties: ascription-invariance
(a value stays equal to itself across a coarsening boundary) and empty-container
distinction (`[] :(LIST OF Number)` is not equal to `[] :(LIST OF Str)`).

The relation is **deliberately intransitive**. A freshly-stamped empty list
relates to both `[] :(LIST OF Number)` and `[] :(LIST OF Str)` through `Any`,
yet those two outer lists are unequal to each other. Transitivity is traded away
on purpose for the two properties above; equality here is never a hash or key
relation, so no map contract is at stake.

## Banned operands: functions and modules

A comparison in which either side — at any depth of the walk — is a function or
a module value is a **structured error**, not `false`. These values are
generative: a module is freshly minted on each evaluation, and a function's
identity is its closure, so structural equality is meaningless. The `==` / `!=`
body renders the error to a `User` diagnostic; the module message points at
`(TYPE OF m1) == (TYPE OF m2)`, which compares module *interfaces* by content
digest — the honest comparison for a generative value.

A shape short-circuit that never reaches a banned cell (a length mismatch, an
unrelated comparability gate) may return unequal first; that asymmetry is
intended — the error fires only when a banned value actually participates in a
pairwise comparison.

## Expression equality

Quoted code is data, so `KExpression` equality is structural over parts: the same
part count, pairwise-equal parts. Keywords, identifiers, and rendered type tokens
compare by their written form; number literals follow IEEE for consistency with
value semantics; nested expressions recurse; list/dict/record *literals* compare
order-sensitively (they are syntax, not the values they would evaluate to). A
`Spliced` part opens both delivery envelopes and compares the carried values by
the value walk — the one place expression equality reaches back into
`value_equal` (and the reason it is cross-lifetime).

The deferred-return duplicate-overload check compares its captured expressions
through this same walk; a banned-shape splice inside a deferred return
conservatively counts as a distinct overload.

## Key normalization

Dict keys are the concrete `KKey`
([`kkey.rs`](../../src/machine/model/values/kkey.rs)) — one of `String`,
`Number`, or `Bool`. Key equality and hashing read the same bits, so the map
contract holds by construction. The key domain is kept NaN-free and
zero-normalized: a `NaN` key is rejected at construction, and `-0.0` is
normalized to `0.0` so the two zeros are one key. Over that domain, `Number` bit
equality coincides with IEEE equality, so key equality agrees with the value-walk
equality wherever both apply.

## Nominal identity, cross-lifetime

Nominal identity (newtypes, tagged sets, module interfaces) compares through
digest-based `KType` equality, which is lifetime-agnostic: `KType<'a>` compares
directly to `KType<'b>`. The predicate suite takes heterogeneous slot and value
lifetimes throughout, so classifying a resolved value against a slot never builds
a mixed-lifetime type and never re-anchors a value across brands — a verdict-only
walk over two independent lifetimes. See the
[type system](../typing/ktype/README.md) for the digest machinery.
