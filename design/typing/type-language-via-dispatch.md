# Type language via dispatch

The type language and the value language share dispatch machinery. A
sigiled expression `:(...)` is a parse-context marker: the parser tags
the inner expression as evaluating to a type rather than a value, but
the inner expression itself routes through the same classifier,
candidate-bucket lookup, and binder admission as any value-side call.
Builtin parameterized types (`LIST`, `MAP`, `FN`) register as
keyworded overloads that produce a `&KType` in the value channel's `Type` arm.

## Sigil surface

```
:(LIST OF Number)
:(MAP Str -> Number)
:(FN (x :Number, y :Str) -> Bool)
:{x :Number, y :Str}
```

The sigil contributes no syntactic structure beyond the marker — the
parser does not fold the inner expression's args into `TypeParams::List`
or any positional collapse. Dispatch sees the raw multi-part expression
through the AST wrapper described below, runs the normal candidate walk
against a registered overload, and the picked overload's body returns a
`&KType` in the value channel's `Type` arm — a structural type, or a nominal
`SetRef` / `Module` / `Signature` identity.

## AST representation

The sigil rides on the slot it occupies via
`ExpressionPart::SigiledTypeExpr(Box<KExpression>)`. The variant wraps
the inner expression as a first-class `KExpression`, so splicing,
lifting, and dispatch-time transformations preserve the type-context
without per-site flag propagation. Pattern-matching against the variant
in the classifier and elsewhere is exhaustive-match-checked by the
compiler — a missed handler is a build error, not a silent fall-through
to the value-side path.

## Fully-uppercase head keywords

`LIST`, `MAP`, `FN` keep parameterized-type construction in
its own candidate bucket, distinct from any user-defined value-side
overload on short connector words. Routing each parameterized type
through its own uppercase head — `[Keyword("LIST"), Keyword("OF"),
Slot]`, `[Keyword("MAP"), Slot, Keyword("->"), Slot]`, etc. — keeps the
buckets narrow even when user-defined functions overload `OF` or `->`
heavily.

`MAP` is the surface keyword for the dict carrier. The underlying type
identity remains `KType::Dict(K, V)`; only the construction surface
changes.

## Function-type sigil

`:(FN (x :Number, y :Str) -> Bool)` declares parameter names at the
sigil surface, symmetric with the FN declaration form and the
value-side rule that function-value calls are named (no positional
`f 1 2` shape). The names round-trip into identity:
`KType::KFunction { params, ret }` carries `params` as a
[parameter `Record<KType>`](ktype/records-and-limits.md#record-fields-and-ktype-hashing),
so `:(FN (a :Number) -> Bool)` and `:(FN (b :Number) -> Bool)` are
distinct types, and the function/return surface re-parses from
`KType::name()` back to the same `KType` — `:(FN () -> Any)`,
`:(FN (xs :(LIST OF Number)) -> Bool)` included. Slot identity is the
record substrate's order-blind equality (same parameters by name and
type regardless of declaration order). Admission (`function_compat`) is
sound function subtyping — contravariant params with width-drop,
covariant return (see [ktype/parameterization-and-variance.md § Variance](ktype/parameterization-and-variance.md#variance)) — so a
value requiring a param the slot doesn't promise is a non-match, while
extra slot params arrive unbound under call-by-name.

The parameter list parses through the shared field-list parser NEWTYPE /
UNION use (`parse_typed_field_list_via_elaborator`), so nested
parameterized param types sub-Dispatch — `:(FN (xs :(LIST OF Number))
-> Bool)` elaborates its element type rather than failing on the bare
identifier.

`:(FN …)` is the only function-type surface, and it covers a functor — a
module-returning function — with no separate spelling:
`:(FN (Ty :Signature) -> Module)`. Capitalized `Type`-token parameter names
(`Ty` for a `:Type` or `:Signature` slot) are admitted by the field-list parser's
`FieldNameKind::IdentifierOrType` policy, so they round-trip into the parameter
`Record<KType>` alongside snake_case ones. A `UNION` schema's variant tags go one
step further — they *must* be capitalized
`Type` tokens (`FieldNameKind::Type`), since a variant is itself a nominal type
(see [user-types.md § Unions dissolve into per-variant newtypes](user-types.md#unions-dissolve-into-per-variant-newtypes));
record fields stay `Identifier`-only.

## Anonymous-union sigil

`:(A | B | C)` is the untagged structural disjunction — a first-class type value,
distinct from a nominal `UNION`. The `|` is a single-member `Unary`-mode
[operator group](../expressions-and-parsing.md#operator-chains), so a run
`A | B | C` reduces to `[Keyword("|"), ListLiteral([A B C])]` and folds every member
in one pass; a two-member run `A | B` stays a plain keyworded call. Both forms are
overloads of the `|` union builtin
([type_union.rs](../../src/builtins/type_union.rs)) that fold their resolved members
through [`KType::union_of`](../../src/machine/model/types/ktype_resolution.rs), so
order never matters and `:(A | A)` collapses to `:A`. A member arrives raw — a bare
`Type` token resolves against scope (parking on a forward reference), a `:(...)`
member sub-dispatches — via a `:KExpression` slot that also admits the reduced
`ListLiteral` ([`accepts_part`](../../src/machine/model/types/ktype_predicates.rs)).
The run has no precedence: `:(LIST OF Number | Str)` does not chain (it is not
slot/keyword-alternating) and must be parenthesized, `:((LIST OF Number) | Str)`.

Untagged union *values* need no construction: a `Number` **is** a valid
`:(Number | Str)` — the builtin constructs only the union *type*. A union-typed slot
admits any value one of its members admits, and a union used as an FN or arm return
type validates but never re-tags, so the value keeps its own runtime type for
downstream type-dispatch (ruling F4,
[finalize.rs](../../src/machine/execute/finalize.rs)). `MATCH` eliminates a union by
that runtime type: each arm head resolves to a `KType`, the admitting arms compete in
the same most-specific-wins tournament
([`ExpressionSignature::most_specific`](../../src/machine/model/types/signature.rs)) that
resolves ordinary overload buckets — boolean-literal and `Result`-tag heads settle first
through an exact pre-pass ranked above every typed arm — and the winner runs (ruling F1,
[find_branch_body_by_type](../../src/builtins/branch_walk.rs)).

## Variant-reference sigil

A single `UNION` variant is named through its union: `:(Maybe Some)` — a
union head followed by a bare variant `Type` token, resolving to the variant's member
`SetRef` ([apply_callable.rs](../../src/machine/execute/dispatch/apply_callable.rs)).
The same `(Union Tag …)` head-call shape constructs (`Maybe (Some 42)`); the two
are disambiguated by body shape — a bare `Type`-token body with no payload is the
variant *reference*, a paren-group payload (`(Some 42)`) newtype-constructs that
member. An unknown variant name at either surface is a schema error listing the
union's members. There is no global `:Some` name and no `.` path operator; the variant
is reachable only through its union. The same sigil names a *sibling* variant of a union
still under seal when it types one of that union's own schema fields (`Node :(Tree Leaf)`):
the elaborator folds the `(Binder Tag)` pair straight to the member's `RecursiveRef`
instead of sub-dispatching, since the producer it would otherwise park on is the seal
awaiting this field; a bare sibling tag (`Node :Leaf`) stays an unknown-type error. See
[user-types.md § Unions dissolve into per-variant newtypes](user-types.md#unions-dissolve-into-per-variant-newtypes).

## Record-type sigil

`:{x :Number, y :Str}` is the structural record type — an identifier-keyed field
schema lowering to [`KType::Record(Record<KType>)`](ktype/records-and-limits.md#record-fields-and-ktype-hashing),
distinct from any nominal struct. The `:` type-sigil anchors to `{` (not only `(`),
and the parser emits a first-class `ExpressionPart::RecordType(<field list>)` part
([frame.rs](../../src/parse/frame.rs)) whose boxed `KExpression` is the bare
`(x :Number, …)` field list. Unlike `:(...)` (which wraps a `SigiledTypeExpr` for the
dispatcher to route), `:{...}` is matched *structurally*: the `DispatchShape::RecordType`
handler folds the field list straight to `KType::Record` via the shared field-list parser
(`elaborate_record_value` in
[dispatch/field_list.rs](../../src/machine/execute/dispatch/field_list.rs),
`FieldNameKind::Identifier`, like NEWTYPE), with no internal type-constructor builtin
behind it. The field list parses through the same `parse_typed_field_list_via_elaborator`
path NEWTYPE / FN use, so nested parameterized field types sub-Dispatch
(`:{xs :(LIST OF Number)}`), while a nested record type `:{inner :{…}}` elaborates
*inline* through the same walker — sharing the elaborator so the outer binder name
threads into the inner record (`NEWTYPE Outer = :{inner :{owner :Outer}}` seals the
inner `owner` to a `SetLocal` back-edge into `Outer`).

A `:{...}` repr is also a distinct `NEWTYPE` overload (`arg("repr", KType::RecordType)`):
the `:RecordType` slot captures the field list raw — the sibling of the `:SigiledTypeExpr`
slot — so the declarator owns the elaboration and threads its own binder name through a
recursive `:{next :Node}`. The two lazy raw-capture slots are part-kind-exact: a `:{…}`
admits only to a `:RecordType` slot and a `:(…)` only to a `:SigiledTypeExpr` slot, so
the overloads stay disjoint.

The record *value* surface is `{x = 1, y = "a"}` (`=` pairs); the brace frame routes
on the first pairing operator, so `:` pairs (`{k: v}`) stay a dict and `=` pairs a
record, mixing the two is a parse error, and an empty `{}` is the empty record. Subtyping over
record values is width/depth — see [ktype/parameterization-and-variance.md § Variance](ktype/parameterization-and-variance.md#variance).

`(x y) FROM r` projects a record value to the named fields
([record_projection.rs](../../src/builtins/record_projection.rs)). Unlike the
type-returning `_OF` dispatcher ops, `FROM` is a plain value builtin: it returns a
`Done` value, `Rc`-sharing the backing record whole and narrowing the carried
field-type record to the named fields — it derives its result type from the literal
field list off the value's own carrier, never routing as a scheduled `OfKind(ProperType)` op.
The field list arrives unevaluated through a `KExpression` slot (bare names only), so
it re-tags a carrier to break an incomparable-arm dispatch tie without name-resolving
the fields.

## Classifier

`classify_dispatch_shape`
([dispatch.rs](../../src/machine/execute/dispatch.rs))
carries a `SigiledTypeExpr` variant whose handler
(`sigiled_type_expr`) tail-replaces the slot with a
`Dispatch` of the wrapped `KExpression`. The inner dispatch sees the
same classifier — there is no separate type-context table — so the
inner expression's parts decide its shape:

- `Keyworded` for the keyworded surface (`:(LIST OF Number)`,
  `:(MAP Str -> Number)`, `:(FN (x :Number) -> Bool)`) served by the
  registered `LIST OF` / `MAP _ -> _` / `FN` overloads in
  [`builtins/parameterized_types.rs`](../../src/builtins/parameterized_types.rs).
  A head with no registered overload — `:(FUNCTOR …)`, say — is an ordinary
  dispatch no-match.
- `BareTypeLeaf` / `BareIdentifier` for single-name sigils
  (`:(Number)`, `:(MyType)`). The `BareTypeLeaf` fast lane is the
  primary caller of `Scope::resolve_type_identifier` — see
  [elaboration.md § Layers](elaboration.md#layers) § Layer 4 for the
  shared resolver bridge.
- `TypeCall` for a leaf-Type head with non-empty rest — routes a
  newtype, union, or `Result` head through its construction primitive
  (`:(MyStruct {x = 1})`, `:(Maybe (Some 42))`) via the shared
  apply-a-callable tail. A constructible `SetRef` identity is the only invocable
  type; `bindings.types` holds no callable, so there is no function-application arm
  here.

A single-part `:(...)` sigil wrapping the whole construction is the
`SigiledTypeExpr` lane that tail-replaces with a `Dispatch` of the inner
expression; a `:(...)` head *followed by* a call body
(`:(MyStruct {x = 1})` as a head) is the `TypeHeadDeferred` lane,
which evaluates the head to a type-shaped value and admits only a
constructible type.

The classifier also carries a `RecordType` variant for a single-part `:{…}`,
separate from the `SigiledTypeExpr` lane. Its handler (`record_type` in
[single_poll.rs](../../src/machine/execute/dispatch/single_poll.rs)) does not
tail-replace with a sub-Dispatch — it folds the field list straight to
`KType::Record`, deferring through a dep-finish only when a field type forward-references
or sub-dispatches. A `:{…}` head in a multi-part expression classifies as
`NonCallableHead` (a record type is a value, not a callable).

The sigil boundary — "the result must ride the value channel's `Type` arm
(a `Signature`, `SetRef`, or any other `&KType`)" — is
enforced implicitly by the consuming slot's KType machinery rather
than by a dedicated tail at the sigil. An `Object`-arm value (number,
instance struct, plain function value, or a **module** — a module is a value)
flowing out of `:(...)`
reaches an `OfKind(Proper)` / `OfKind(Any)` / `OfKind(Signature)` slot
and surfaces a standard `TypeMismatch`. That is why type-language dispatch
(`:(LIST OF int_ord)`) refuses a module head: a module is a value and names no
type, so it reaches type position only through `TYPE OF` (see
[modules.md § Modules in type position](modules.md#modules-in-type-position-type-of)).
The sigil handler itself does
no extra check; the slot-type rails are the single source of truth.

Every parameterized type rides one surface: the keyworded sigil
(`:(LIST OF Number)`, `:(MAP K -> V)`, `:(FN … -> R)`), served by the
type-constructor overloads. The field-walker inside `typed_field_list`
handles the sigil embedded in `NEWTYPE` / `UNION` field schemas through a
single path. Keyworded shapes (`:(LIST OF Tree)`, `:(MAP Tree -> _)`)
sub-Dispatch through the standalone dispatcher, which carries no threaded
binder set, so `rewrite_threaded_self_refs` first rewrites every threaded
self / group-sibling reference to a `Future(Carried::Type(RecursiveRef(name)))`
carrier — the same `Type`-arm transport `:(LIST OF Number)` rides — before the
sub-Dispatch. This lowers `NEWTYPE Tree = :{children :(LIST OF Tree)}`'s
field to `List(RecursiveRef("Tree"))`, which seals to `List(SetLocal(_))` at
the member's finalize, rather than parking on `Tree`'s own placeholder and
deadlocking the scheduler.

## Binder install: name-keyed vs bucket-keyed

`LET`, `NEWTYPE`, `UNION`, `SIG`, and `MODULE` register a single name
binding via a `binder_name` extractor and ride the name-keyed
placeholder channel. `FN` registers an *overload* in a
function bucket via a `binder_bucket` extractor — and crucially,
*not* a `binder_name`. The two channels are reflected at the
submission walk as `BinderKey::Name(String)` and
`BinderKey::Bucket(UntypedKey)` (see
[`scheduler/alloc.rs`](../../workgraph/src/scheduler/alloc.rs)),
mutually exclusive per binder.

The bucket-keyed channel admits *sibling* overloads under one head
keyword. Two `FN (PICK xs :A) ...` / `FN (PICK xs :B) ...`
declarations each install a distinct entry into the same
`pending_overloads[bucket]` per-bucket vec; the earlier-index entry
is the wake target for a consumer parking on the bucket, and the
later-index siblings remain pending until their own finalize. On
each producer's finalize, only its own entry is removed; if a parked
consumer's first wake doesn't deliver an admitting overload, the
consumer re-dispatches and either picks from the now-live
`functions[bucket]` or re-parks on the next-earliest pending sibling
(see [execution/README.md § Dispatch-time name
placeholders](../execution/name-placeholders.md#dispatch-time-name-placeholders)).
A name-keyed install would collide on the second sibling — both
`PICK` binders trying to claim `placeholders[PICK]` — which is why
FN does not install on the name channel.
