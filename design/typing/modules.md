# Modules

Koan's abstraction unit is the *module*: a bundle of types and operations behind
a signature, with first-class module values and modular implicits providing
ergonomic generic dispatch. [open-work.md](open-work.md) carries the work that
remains.

The motivation is uniformity: multi-parameter dispatch, higher-kinded
abstraction, and representation hiding all fall out of one mechanism rather
than sitting in three.

The module surface is described across several files: this one covers
structures, signatures, and first-class module values; [functors.md](functors.md)
covers parametric modules and higher-kinded slots; [implicits.md](implicits.md)
covers modular implicits and axiom-checked coherence;
[scheduler.md](scheduler.md) covers how inference and search ride the same
scheduler that runs value evaluation.

## Structures and signatures

A **structure** (declared with `MODULE`) bundles type definitions, values,
and functions:

```
MODULE int_ord = ((LET Carrier = Number) (LET compare = FN ...))
```

A **signature** (declared with `SIG`) is a module type — an interface
specifying what a structure must contain:

```
SIG Ordered = ((TYPE Carrier) (VAL compare :(FN :{x :Carrier, y :Carrier} -> Number)))
```

A module is a value, so a module name is an **Identifier** token: snake_case,
lowercase-leading (`int_ord`, `int_set`). A signature *is* a type, so a signature
name is a **Type** token — uppercase-leading plus at least one lowercase character,
with no suffix (`Ordered`, `Set`). The Type-token namespace is therefore exactly
the set of names that can type a field. `MODULE int_ord = …` under a Type-token
name is an error carrying the snake_case respelling
([`module_def.rs`](../../src/builtins/module_def.rs)), as is a `LET` of a module RHS
under one — and beneath both, the binding maps refuse the crossing outright
([tokens.md § Token class is a binding rule](tokens.md#token-class-is-a-binding-rule-not-just-a-lexical-one)).
The rule reaches **parameters** too: a parameter's name picks its universe, not the
argument it is handed, so a module-valued parameter spells snake_case (`er`) exactly as a
module does, and handing a module to a Type-token parameter is a bind-time error. The
counterexample worth keeping in view is `Er :Signature` — a *signature*-valued parameter
carries a type-language value, so it keeps the Type-token spelling.
Abstract types declared inside a signature take the Type-token spelling
too — the convention is `Carrier` for the principal abstract type, with additional
abstract types named `Elt`, `Key`, `Val`, etc. when more than one is needed. The
token-class rule that distinguishes `MODULE` (keyword: ≥2 uppercase, no lowercase)
from `Ordered` (Type token) and `int_ord` (Identifier) is described in
[tokens.md](tokens.md).

SIG bodies accept four declarators, split by what a satisfying module must
supply:

- `TYPE <TypeName>` declares an **abstract** type member — a witness-less slot
  the module supplies at any concrete type. `TYPE (<Param>… AS <Name>)` is the
  higher-kinded form, taking one or more named parameters
  (see [Higher-kinded type slots](functors.md#higher-kinded-type-slots)).
  `TYPE` is meaningful only inside a SIG body; the implementation lives at
  [`type_decl.rs`](../../src/builtins/type_decl.rs).
- `LET <TypeName> = <expr>` declares a **manifest** type member — a slot fixed
  to the RHS type. A satisfying module's member must equal it. Inside a SIG body
  the invariant is `=`-iff-manifest: a Type-class `LET` binds the concrete RHS
  (no abstract re-tag), and abstract members use `TYPE`, which has no RHS.
- `(VAL <name> :<TypeExpr>)` declares a value slot: the canonical surface for
  naming an operation the signature requires, with the slot's declared type
  recorded explicitly rather than inferred from an example value.
- `(FN (<head>) -> <Return>)` — a bodyless `FN` head — declares a **keyworded
  member**: an entry in the module's dispatch buckets, the half of a module's
  callable surface `VAL` cannot name. See
  [Keyworded members](#keyworded-members) below.

A SIG body's declarators write three separate channels. The decl scope's `types` map records
each `TYPE <Name>` abstract member and each `LET <Name> = <Type>` manifest member under its
Type-token name — genuine type bindings. Each `VAL <name> :<Type>` value slot instead records
its declared type into a **slot collector** on the decl scope
([`Scope::sig_slot`](../../src/machine/core/scope.rs) / `sig_value_slots`), keyed by the slot's
value name — a schema in progress, off the binding map. Each bodyless `FN` head records its
`(params) -> ret` type into the slot collector's twin, a **keyworded collector**
(`Scope::write_sig_keyworded` / `sig_keyworded_members`) keyed by the head's untyped bucket key.
At SIG finish all three channels project once
into the signature's stored [`SigSchema`](../../src/machine/model/types/sig_schema.rs): the
`types` map splits by representation into abstract vs manifest members, the slot collector
supplies `value_slots`, and the keyworded collector supplies `keyworded`. Because neither
collector enters `types`, the token-class partition needs
no exemption ([elaboration.md § Binding-map partition](elaboration.md#binding-map-partition)) and
a SIG body's `Bindings` is an ordinary `Bindings::new()`.

`VAL`, `TYPE` and the bodyless `FN` head are meaningful only inside a SIG body; outside it the
declarator is unbound and a bodyless head is an error naming the definition spelling. The
lowercase-name `(LET name = <value>)` form is
rejected inside SIG bodies with a diagnostic directing to `VAL`, and a bare `FN` *with* a body
there is rejected with one directing to the bodyless head. The implementation lives at
[`val_decl.rs`](../../src/builtins/val_decl.rs); ascription
([`ascribe.rs`](../../src/builtins/ascribe.rs)) checks a module against a signature
through the **signature-subtyping relation**
([`sig_schema.rs`](../../src/machine/model/types/sig_schema.rs)), so a VAL slot's
declared type is checked structurally: the module's member type must be covariantly
compatible with the slot's declared type (see §"Satisfaction and `WITH`" below).

Structures can be **ascribed** to signatures via two operators that differ
only by a whitespace gap in the visual rendering, expressing "you can see
through this":

```
LET int_ord_view     = (int_ord :! Ordered)   -- transparent
LET int_ord_abstract = (int_ord :| Ordered)   -- opaque
```

An ascription yields a module, so its `LET` name is snake_case like any other
module binding. *Transparent ascription* (`:!`) checks that the structure satisfies
the signature but leaves type definitions visible: `int_ord_view.Carrier` resolves to
`Number` just as `int_ord.Carrier` does. *Opaque ascription* (`:|`) additionally
hides the representation: outside the ascription, `int_ord_abstract.Carrier` is
**not** the same type as `Number`, even though that's its underlying
definition. Type checking forbids passing an `int_ord_abstract.Carrier` value to
anything expecting a `Number` — the abstraction barrier is enforced.

**An ascription view is a narrowing, on both operators.** A view surfaces exactly
the members its signature declares — type members, value slots and keyworded
members — and nothing else, ML-style: a source member the signature omits is
absent from the view, not merely untyped by it. Width subtyping is a property of
the **matching relation** — a module that binds more than the signature names
still satisfies it — never of the view the match produces. Transparency is about
the *types* a declared member reads at, not about how much of the source shows
through, so `:!` prunes exactly as `:|` does. Pruning shapes the view alone: the
source module still binds everything it always did.

Opaque ascription is **generative**: each application mints a fresh
`KType::AbstractType { source: view.scope_id(), name }` per declared abstract
type, where `view` is the freshly allocated child module the ascription
introduces. `AbstractType`'s `source` is a plain
[`ScopeId`](../../src/machine/core/scope_id.rs), so the variant is owned data
carrying no `&Module`. It has two **minting sites** — this per-call ascription
module, and the SIG decl scope for a declaration-time member (below) — which the
representation no longer distinguishes: they differ only in *which* scope id
`source` names, and that is exactly what keeps them apart. Manual `PartialEq`
compares `(source, name)`, so two opaque ascriptions of the same source module
yield distinct `scope_id`s and therefore distinct types that cannot be confused,
while two `KType::AbstractType` carriers minted from the same ascription compare
equal. The carrier lives in
[`KType`](../../src/machine/model/types/ktype.rs); the operators are registered as
ordinary builtins in [`ascribe.rs`](../../src/builtins/ascribe.rs).

### VAL-slot reads carry the abstract member identity

A SIG-local abstract-type binding stays *named* end to end, so a slot read
through an opaque view reports the abstract type rather than the underlying
representation — on every slot shape and every read surface. Two sites cooperate:
the declaration that keeps the member named, and the ascription that builds a view
whose members already inhabit its own types.

A `TYPE Carrier` declaration ([`type_decl.rs`](../../src/builtins/type_decl.rs)) binds
the name-bearing `KType::AbstractType { source: <decl scope id>, name }`, so a
later `VAL zero :Carrier` records that `zero` *names* the abstract member `Carrier`. The
higher-kinded `TYPE (Elem AS Wrap)` binds an `AbstractType` too, its `param_names`
carrying the declared parameters, so ascription's per-call constructor mint preserves
the parameterization. A manifest
`LET Tag = Number` binds the concrete `Number` — it carries no abstract identity,
and a `VAL x :Tag` slot reads through concretely. Classification is by
*representation*, not name class:
[`sig_schema.rs`](../../src/machine/model/types/sig_schema.rs)'s `is_abstract_sig_member`
is exactly `matches!(kt, KType::AbstractType { .. })` — an abstract member of either
order is that one variant, and everything else (a manifest binding, a minted
constructor family) is manifest. Outer aliases and builtin annotations
(`:Number`, an outer `LET MyAlias = Number`) stay concrete.

Opaque ascription ([`ascribe.rs`](../../src/builtins/ascribe.rs)'s `body_opaque`)
seeds the view's fresh child scope with the view's own type interface — a per-call
`AbstractType` mint for each abstract member of the signature, each manifest member
at its fixed `KType` — and reads that table back into `type_members`, which is
therefore a mirror of the scope (the seeding happens inside
[`Scope::alloc_module_view`](../../src/machine/core/scope/registry.rs), the door that
births the scope, because the mints need the newborn scope's id as their nonce and
only the birthing door may mint the
[`WriteGate`](../../src/machine/core/bindings/gate.rs) for an unpublished scope). What the seeded table does *not* hold — the source's
representation type behind an abstract member — is unreachable through the view by
construction rather than by masking. The map is gathered into a
`ModuleDraft` and frozen into the view module at construction — a built module has
nothing to write — and the per-application generativity nonce is the view scope's own id, which
`Module::scope_id` reports back off the finished value.

**Members are born coerced.** The same door replays the source module's bindings
into the view's child scope, and a member whose SIG-declared slot type substitutes
*differently* under the view's mints than under the source module's own bindings is
not replayed as the source's seal: it is rewritten into the view scope's region so
that it inhabits the view's types
([`Scope::seal_coerced_member`](../../src/machine/core/scope/reach.rs)). Coercing
once at construction, rather than patching each read, is what makes every read
surface agree by construction — ATTR, a `USING` window borrowing that very binding
table, a dynamic read, and a functor's deferred return alike.

The rewrite is the walk in
[`coerce.rs`](../../src/machine/model/values/coerce.rs), which recurses on the
SIG-**declared** slot type — never on the two substituted types in lockstep, since
union interning canonicalizes member order and positional correspondence between two
separate substitutions is therefore not stable. Its arms mirror
`substitute_sig_members`, so the walk is finite for the same reason substitution is:

- a position whose two substitutions **agree** returns the value untouched — the
  whole of a concrete slot (`VAL compare :Number`), a manifest-typed slot
  (`VAL x :Tag` after `LET Tag = Number`), and every sub-position naming no abstract
  member;
- an `AbstractType` or `ConstructorApply` position **re-tags**: the value's identity
  handle is replaced with the view's substitution, sharing the payload substrate
  (`KObject::wrapped_peel` collapses one layer, so the single-layer invariant holds);
- a list, dict or record is **rebuilt** cell by cell against its declared element,
  value or field type and re-stamped to the substituted container type. A declared
  *dict key* type naming an abstract member re-stamps the dict's type only: a `KKey`
  is a concrete scalar with no type identity to carry, so a key read back off a
  coerced dict is a bare scalar;
- a `KFunction` position takes the **eta-wrapper** below rather than recursing into
  the value — a callable crosses the barrier per call, at its own boundary;
- a `Union` picks the declared member whose source-side substitution the value
  inhabits, and coerces by that member alone;
- a nested `Signature` position takes the **module boundary** below — the member is a
  module, and it is rebuilt as a view of itself.

A member the signature does not name is not replayed at all — it has no declared slot
type to coerce against and no place in the view. Transparent `:!` runs this same
construction, seeding each abstract member with the source's **own binding** for it
rather than a mint: the two substitutions of every slot then agree, so the coercion
plan is the identity, nothing is coerced, every declared member replays verbatim, and
transparent reads stay concrete. Transparency is therefore a property of the seeding,
not a second code path — `body_opaque` and `body_transparent` are the same function at
two seedings. The two
bindings a walk rewrites between ride as a `MemberCoercion`
([`sig_schema.rs`](../../src/machine/model/types/sig_schema.rs)): each table is
interned as the `Signature` handle whose manifest members *are* the table, which
makes the carrier `Copy` and lifetime-free — what lets a coercion plan sit inside a
[`Body`](../../src/machine/core/kfunction/body.rs) and inside a sealed return
obligation, neither of which may name a region.

**The function boundary.** A callable filling a slot whose declared `FN` type names
an abstract member is wrapped rather than rewritten. The wrapper's signature is the
underlying's with each declared parameter re-typed to the view's substitution, so
dispatch and argument validation admit the reading side's types and a source-typed
argument is a mismatch at the wrapper itself. Its `Body::CoercedDelegate` carries the
underlying callable and the coercion plan: the invoke coerces each argument inward,
runs the *underlying's* body, and installs a `ReturnContract::Coerced` whose
obligation checks the result against the underlying's side of the barrier and then
rewrites it to the view's at the lift boundary
([`finalize.rs`](../../src/machine/execute/finalize.rs)'s rebuild disposition — the
same single re-anchor an ordinary declared-return re-stamp takes). The wrapper is
born at the underlying's **own captured scope**, so both callables live in one
region: a callable's residence *is* its captured scope's region, the invariant every
callable read depends on.

**The module boundary.** A slot type may name another signature — `VAL subs :(LIST OF
(Inner WITH {Item = Elt}))` says "a list of `Inner`s over *my* element type". The member
filling such a slot is itself a module, and it is rebuilt as an opaque view **of itself**
through the same [`Scope::alloc_module_view`](../../src/machine/core/scope/registry.rs)
door the outer view took, carrying the outer coercion plan
([`coerce_module`](../../src/machine/model/values/coerce.rs)). Its plan is built through the
same `ViewMembers::of_schema` builder the outer boundary uses, so the two cannot drift: the
nested view surfaces exactly what the nested signature declares — an abstract member at the
source module's own binding, a manifest member at the outer view's substitution, its value
slots and keyworded members born coerced by the replay — so the nested
member reports the outer view's `Elt` mint rather than the source's representation, on the
same terms every other slot shape does, and it narrows exactly as the outer view does.
Nothing is
minted at the nested boundary: a nested view's abstract identities *are* the outer view's
mints, arriving through the substitution. The view is born at the source module's own child
scope, so it lives in the source's region exactly as a function wrapper lives in its
underlying's, and the enclosing coercion's pin keeps that region for the product's life. The
nested view's self-sig is the plain `raw_self_sig` derivation, because a scope born holding
coerced values already reports the view's identities off each member. Nesting composes: a
nested view's own replay re-enters the walk, so a signature inside a signature inside a
container coerces at every depth.

**Substitution recurses through a nested signature.** The three schema walks in
[`sig_schema.rs`](../../src/machine/model/types/sig_schema.rs) — `substitute_sig_members`,
`references_sig_member` and `canonicalize_binder` — descend into a nested `Signature`'s own
manifest members and value slots like any other compound, so a reference to the enclosing
signature's `Elt` inside one is rewritten, is reported by the fast-path probe, and is
re-sourced to `ScopeId::SENTINEL` at projection (which is what makes two textually identical
declarations carrying a nested signature intern to one type). **Shadowing is by name**: every
projected SIG canonicalizes its own binders to `ScopeId::SENTINEL`, so `source` cannot tell an
inner binder from an outer one, and a nested schema's own abstract-member names are subtracted
from the substitution before the descent. A nested binder is therefore inexpressible from the
enclosing signature and untouched by the outer view's plan — the nested member keeps its own
binding for that name. The digest walk
([`type_digest.rs`](../../src/machine/model/types/type_digest.rs)) deliberately stops at a
nested `Signature` instead: the nested handle inside a projected schema is already canonical,
so its own digest is exact, and collapsing the name leaves inside it would conflate an inner
binder with an outer reference of the same name. A signature is not admissible in a `VAL`
slot directly — `VAL`'s type slot is `KKind::ProperType` — so a module-typed member is
reachable only nested inside a container.

**Satisfaction and `WITH`.** Satisfaction is a **signature-subtyping** check
([`sig_schema.rs`](../../src/machine/model/types/sig_schema.rs)). Every module carries a
principal **self-sig** — a [`SigSchema`](../../src/machine/model/types/sig_schema.rs) of its
abstract members (always none — `TYPE` is SIG-body-only), manifest type members,
value-slot types, and its **keyworded surface** (every dispatch bucket the body registered, each
overload named by the `(params) -> ret` type its callable reports) — derived from the members
construction gathered, interned *before* the module
value exists, and carried on it immutably (`SigSchema::raw_self_sig` is the whole derivation for a
bare construction). A signature *is* a
`SigSchema` (`WITH` pins are already folded into it at intern time). Ascription (`check_satisfies`,
run for both `:|` and `:!`) holds iff `module.self_sig <: sig-schema` under `sig_subtype`:
`Sub <: Super` iff `Sub` supplies every member `Super` names (width — extra `Sub` members are
ignored), with each manifest member *equal*, each abstract member present at the matching
kind and over the same parameter names (a first-order slot needs a proper type or
first-order member; a higher-kinded `TYPE (Elem AS Wrap)` slot needs a constructor whose
parameter-name *set* equals the slot's — see
[functors.md § Higher-kinded type slots](functors.md#higher-kinded-type-slots)), and each value slot
covariantly compatible — the module's member type must be `satisfied_by`-admissible for the
slot's declared type, after the slot's references to `Super`'s abstract members are substituted
with `Sub`'s bindings for them — and each keyworded member satisfied by a most-specific overload
under the same key ([Keyworded members](#keyworded-members) below). Each ascription view is born
carrying a self-sig recording those
substituted slot and keyworded types, so a view structurally satisfies its own signature. The
result is
memoized in the type registry under `Relation::SigSatisfies`, keyed on the two schema content
digests (a pure cache — types are immutable).

Dispatch matching of a `:Sig` slot runs the same structural check ascription asserts
([`Module::satisfies_sig_content`](../../src/machine/model/values/module.rs), memoized on the
pair of schema content digests, with a content-equality fast path that records nothing). A
`WITH` pin is a manifest member of the folded schema, so pinned-slot agreement is the
manifest-equality leg of that one check. Ascription is
assertion plus view construction, never an admission gate: an unascribed module whose self-sig
satisfies a signature is admitted by that signature's slot directly.
`WITH` pins abstract slots by **folding each pin into the schema**
([`SigSchema::fold_pins`](../../src/machine/model/types/sig_schema.rs)): the pinned abstract
member becomes a manifest member, and slot references to it substitute to the pinned type.
Specialization therefore accumulates across chained `WITH`, is order-independent, and admits
no second spelling — `Ordered WITH {Carrier = Number}` interns the same type as a SIG declaring
`Carrier = Number` outright (or a structurally identical module self-sig). A pin naming a slot
already fixed — a manifest member, including one an earlier `WITH` folded — is normalized away
when it equals the fixed type (leaving signature identity unchanged) and is a type error when
it differs ([`type_ops/with.rs`](../../src/builtins/type_ops/with.rs)).

ATTR's `access_module_member` ([`attr.rs`](../../src/builtins/attr.rs)) reads a
value-side member exactly as it is bound, and patches nothing: the view's scope
already holds the coerced value. So `(int_ord_view.zero)` reads as the per-call
abstract `Type` (opaque), not the underlying `Number` — a
[`KObject::Wrapped`](../../src/machine/model/values/kobject.rs) carrier whose
`type_id` is that identity, the same `Wrapped` variant NEWTYPE uses and
distinguished by its `type_id`'s KType — and a functor body
`(FN (GET_ZERO er :WithZero) -> er.Carrier = (er.zero))` whose return type is the
per-call abstract member admits the slot read. The coerced member lives in the view
scope's own region (declaration-stable), whose composition retains the source
module's region for the payload substrate a re-tag shares, so both outlive any lift
or deep-clone of the read value into a per-call functor region.

Opaque ascription is the type-abstraction primitive. It replaces the
newtype-with-private-fields pattern that a trait system would need.

### Keyworded members

A module's callable surface has two halves. A `LET pure = FN (PURE x :Number) …`
binds `pure` in `data` *and* registers `PURE` in the module's dispatch buckets; a
bare `FN (PURE x :Number) …` registers only the bucket. `VAL pure :(FN …)` names the
value half. The **bodyless `FN` head** names the other:

```
SIG Box = ((TYPE Elt) (VAL zero :Elt) (FN (PURE x :Elt) -> Elt))
```

The head is the definition form's head with the `= (<body>)` dropped, and it parses
through the definition form's own path
([`fn_def.rs`](../../src/builtins/fn_def.rs)), so a declaration and the definition
that satisfies it derive their bucket key and slot types from one implementation.
Because it takes no body, its bucket key `[FN, Slot, ->, Slot]` is shorter than every
definition spelling and the two never compete; the key *is* shared with the
function-type expression `FN :{…} -> <Ret>`
([`parameterized_types.rs`](../../src/builtins/parameterized_types.rs)), and the
signature slot tells them apart — a `(…)` head is captured raw by the bucket's
lazy-slot entry and reaches the declarator, a `:{…}` record type resolves and reaches
the type form. A head with no fixed token has no bucket to declare, and a return type
naming a parameter (`-> er.Carrier`) is a per-call elaboration a declaration has no
call to run; both are refused at the declaration rather than at an ascription later.

**An overload's identity** is its untyped bucket key plus an interned `KFunction`
type — a record of *named* argument types and a return type. Parameter names are
interface, exactly as in a `VAL` FN slot; slot order within the key is presentation.
Several declarations may share one key, and the key's overloads are a *set*, held in a
canonical order so two signatures declaring the same overloads intern to one type. An
exact duplicate is a `Rebind`; a same-key declaration at a different type joins the
set.

The keyworded channel is signature content: it feeds the schema's content digest, is
rendered after the value slots in a signature's name
(`SIG (zero: Elt, (PURE x :Elt) -> Elt)`), rides `TYPE OF`, folds through `WITH` pins
like any other declared type (two overloads that collapse to one under a pin become
one), and intersects in a signature join — per shared key, overloads pair by
parameter-name set, and one with no unique partner drops.

**Satisfaction mirrors dispatch resolution.** For each declared overload,
`sig_subtype` finds the module overloads under the same key that *satisfy* it — the
same covariant `KFunction` rule a value slot's function type is checked by — and takes
the most specific of them. Three failures are named at the head that wanted them: no
bucket under the key (`MissingKeyworded`), a bucket whose overloads all fail
(`KeywordedMismatch`), and an incomparable tie among satisfiers
(`AmbiguousKeyworded`) — the keyworded reading of a dispatch ambiguity, raised at the
ascription rather than at the call.

"Most specific" is **the same code dispatch uses**, not a second lattice. The per-slot
fold behind `ExpressionSignature::specificity_vs` — the ranking dispatch applies to
co-bucket call shapes — is extracted as `specificity_over`, and both callers route
through it: `specificity_vs` pairs two live signatures' argument slots positionally,
`fn_type_specificity` pairs two declared function types' parameters *by name*
([`signature.rs`](../../src/machine/model/types/signature.rs)). Returns are excluded,
because dispatch never selects on them. A test pins the agreement directly: the
type-level verdict equals `specificity_vs` on the live signatures.

**A view publishes the selection.** The ascription replay installs, per declared
overload, the one source overload that satisfies it — wrapped by the same
`coerce_function` eta-wrapper a `VAL` FN slot takes where the coercion tables say the
declared type crosses the barrier, and as the source's own seal where it does not. So
a keyworded call through an opaque view coerces exactly as the value-lane read of the
same function does: results carry the view's types, a source-typed argument is a
mismatch at the wrapper. Selection and satisfaction run
`select_keyworded_satisfier` ([`sig_schema.rs`](../../src/machine/model/types/sig_schema.rs)),
one implementation, so the member the check admitted is the member the view installs.
A module overload no declared member selects is **not** installed: under a signature
declaring `(PICK x :Number)`, a module's `(PICK x :Any)` is unreachable through the
view. Two declared overloads may legitimately select one source overload; the pair
they publish under dedupes them.

A keyworded member is reached by dispatch, so it is called through a
`USING <view> SCOPE` window rather than by qualified name — see
[Block-scoped opening](#block-scoped-opening-using--scope).

### Module bodies announce their type declarations

Before it runs any body statement, `MODULE` pre-scans the body's **top-level**
statements for type declarations and announces every name it finds
([`announce_type_members`](../../src/machine/model/binder.rs)). The announcement rides
the body's child scope as its ambient declaration window
([`ScopeKind::Module`](../../src/machine/core/scope.rs)'s `window`), so an announced
name is visible to every statement of the body regardless of order — which is what
makes a plain module the construct for mutually-recursive nominals:

```
MODULE listy = (
  NEWTYPE Cell = :{head :Number, tail :Rest}
  NEWTYPE Rest = :{next :(Cell | Null)}
)
```

`GROUP` runs the same scan through the same door
([`group_def.rs`](../../src/builtins/group_def.rs)) — a group *is* a module, so it
hosts a mutually-recursive group in its body exactly as `MODULE` does.

A statement announces iff its own parse-time binder key is the `NEWTYPE <Name> = _` or
`UNION <Name> = _` spec, matched on the full bucket key, so a user overload sharing a
head keyword is excluded. A `UNION` announces one member per variant tag, owned by its
binder: an owned member never reaches `bindings.types` and is therefore absent from
`Module::type_members`, since a variant is constructed through its binder
(`Tree.Node …`) or named through member projection (`:(Tree.Node)`), never as a
module member of its own. The whole group's `types` writes land when the last
announced member fills; a member the body never fills is a typed `ShapeError` at the
module's finish, not a hang. The type-side mechanics — window representations, the
consumer/declarator split, the seal — are in
[user-types.md § Mutual recursion](user-types.md#mutual-recursion--the-module-body-announcement).

Announcement is a *module* property, not a global scan rule: the program's own top
level announces nothing, so a mutually-recursive group written there is an ordinary
forward-reference miss and takes the module wrapper.

Announced members are ordinary type members of the finished module, so they reach a
use site by qualification or through a `USING` window
([Block-scoped opening](#block-scoped-opening-using--scope) below).

## First-class modules

The type language is first-class; modules and signatures live there. A
module value rides the value channel's Object arm as
[`KObject::Module`](../../src/machine/model/values/kobject.rs), typed by its
principal signature — `ktype()` reports
`KType::Signature { content, .. }` sharing the module's sealed self-sig content, so dispatch
trusts the carried self-sig. A signature value rides the
[`Carried::Type`](../../src/machine/model/values/carried.rs) arm as
`KType::Signature { schema, .. }` — the same arm that carries `Number`,
`Str`, and builtin type values. A module value flows through `LET`, ATTR, and function
calls like any other value: there is no separate pack/unpack form, no
`(module M)` construction syntax, and no `(val m)` projection. A module
named in expression position evaluates to its Object-arm value, and `m.compare` is
ordinary attribute access — ATTR projects through the `KObject::Module`
carrier to reach `module.access_module_member(field)`. Member access is
**module-own**: one classified
[`Bindings::lookup_member`](../../src/machine/core/bindings.rs) reads the
module's own `data` then `types` and returns the value-or-type in a single pass
(the `data`/`types` cross-kind exclusion makes the result unambiguous), so a name
that isn't a declared member is a missing member — it does **not** fall through to
a builtin type or a lexically enclosing binding. `int_ord.Carrier` therefore resolves
only when `int_ord` declares a `Carrier` member (the `LET Carrier = …` convention),
never to a lexically enclosing binding. The module-own rule holds even for a spelling
that collides with a builtin: `int_ord.Type` reads module-own too, and since `Type`
is an unshadowable builtin meta-type no module can declare that member, so it is a
missing member, never the builtin. Signature member access
(`access_type_member` over `KType::Signature`) answers from the signature type's own owned
schema — a manifest or abstract type member first, then, **under the type sigil only**, a
declared `VAL` slot's type — with no decl-scope lookup, so a signature projects exactly the
interface its content names. `Ordered.Carrier` names the abstract member bare, while the
value-slot read needs `:(Ordered.compare)`: `compare` is a value token, and a value token names
a type only where the surface says so (see
[tokens.md § A value token names a type only under the sigil](tokens.md#a-value-token-names-a-type-only-under-the-sigil)).

`MODULE` binds **value-side**: it takes an `Identifier` name part, installs a
`BindKind::Value` placeholder, and finalize allocates the Object-arm module value and
binds it into `bindings.data` through
[`Scope::bind_module`](../../src/machine/core/scope.rs) — a fused door that derives
the module's stored reach off its child scope directly (never by walking the built
value) and allocates the value under that same evidence. `LET view = (int_ord :|
Ordered)` binds a module RHS the same way: a module is a value, so it lands in
`data`, and the `data` / `types` cross-kind exclusion keeps it out of `types`. A
module-typed FN parameter binds value-side too, through the ordinary Object-arm
parameter door. **No binding door installs a module into `bindings.types`**, and
`KType` carries no module variant.

`SIG` declarations still bind **type-side**: finalize installs
`KType::Signature { schema, .. }` into `bindings.types` via
[`Scope::register_type_upsert`](../../src/machine/core/scope.rs), and a `LET Sortable =
Ordered` signature alias routes through `register_type` against that entry. A
signature identity rides the `Type` arm, surfaced from the type entry on demand by
[`Scope::resolve_type_identifier`](../../src/machine/execute/decide/resolve_type_identifier.rs).

A module name is an Identifier token, so the resolver ladder reads it on the
**value** channel like any other value name: no ladder arm is keyed to "a Type token
that turns out to name a module", and a module's value write clears no Type-kind
placeholder. ATTR's `body_module` reads its module receiver off the Object arm, so
`int_ord.Carrier` and `er.Carrier` alike project off the module *value*; there is no
type-side module projection anywhere.

### Modules in type position: `TYPE OF`

A module name names no type. It is a value token, so a slot annotation `:int_ord`
does not even lex — `:` takes a Type token, and the parse error names the
replacement spelling. The one door from a module to type position is the `TYPE OF`
builtin ([`type_ops/type_of.rs`](../../src/builtins/type_ops/type_of.rs)):

```
FN (TAKE_ORD m :(TYPE OF int_ord)) -> Number = (m.zero)
FN (USE_ORD er :Ordered) -> :(TYPE OF er) = (er)
LET SetType = (TYPE OF int_set)
```

`TYPE OF <value>` yields the type the value reports for itself (`KObject::ktype()`)
as an ordinary type value — `TYPE OF 5` is `Number`, `TYPE OF xs` is
`LIST OF Number` — so it is general over the value channel, not a module-specific
form. Applied to a module it yields that module's **principal signature**
(`KType::Signature { schema: <m's self-sig> }`), which is how a
module reaches a slot, a return, or a `LET` type alias. Its `value` slot is
`KType::Any`, which admits both channels, so a *type* argument reaches the body and
is refused there with a diagnostic rather than falling through dispatch as a miss. The result
is built at the fold brand from the argument's own carrier, so the type it produces
borrows exactly the region the value lives in — a module minted in a function's
per-call region included.

Three consequences:

- `-> :(TYPE OF er)`, where `er` is a module-valued parameter, is a deferred return
  meaning "returns a module satisfying `er`'s interface". The per-call contract is
  the argument module's self-sig, cloned into the captured-scope region through the
  single type door ([`home_return_type`](../../src/machine/core/kfunction/exec.rs)) —
  a `KType` owns all its content, so the clone borrows only its destination and
  needs no reach evidence. The argument module need not live in
  the captured region: a module minted in a functor's per-call
  region rides the return like a root-bound one (see
  [per-call-region/lifecycle.md](../per-call-region/lifecycle.md)).
- `m :(TYPE OF int_ord)` is a **structural** slot: it admits any module whose
  self-sig satisfies `int_ord`'s, not only `int_ord` itself. Admission runs the same
  `sig_subtype` walk every signature slot runs, so ascription is never required.
- `LET SetType = (TYPE OF int_set)` binds the self-sig as an ordinary type alias, on
  the `types` entry, carrying the module's reach — so a later `:SetType` slot replays
  a reach that still pins the module's region.

A return slot naming a value directly (`-> er`) is an error rather than a silent
widening: no member of the return slot's carrier union admits a value name, and the
[dispatch-miss diagnosis table](../../src/machine/model/miss_diagnostics.rs) names the
`:(TYPE OF er)` respelling on the miss — whether `er` is unbound in the defining scope
(the parameter case) or bound to a value there. A return-type elaboration miss is surfaced
instead of falling back to `Any`. All of this is pinned by
[`module_head_in_type_position`](../../src/builtins/fn_def/tests/functor/module_head_in_type_position.rs)
and [`type_of/tests.rs`](../../src/builtins/type_ops/type_of/tests.rs).

Each [`Module`](../../src/machine/model/values/module.rs) seals a principal self-sig
([`SigSchema`](../../src/machine/model/types/sig_schema.rs)) at creation — the immutable
structural type the satisfaction relation reads (see §"Satisfaction and `WITH`").
The `Signature` node `{ schema: SigSchema, schema_digest }` carries the owned
schema and its content digest — no binder, no label, and no pin set (`WITH` folds its pins
into the schema before interning). There
is one kind of signature type: a `SIG` declaration, a module value's principal self-sig, and the
empty `:Module` interface differ only in the schema, not in node kind. A `KType` is a `Copy`
registry handle, so it holds no region pointer.
The `AbstractType { source, name, param_names, nonce }` node carries an abstract-type member: its
`source` names the binder (canonicalized to `ScopeId::SENTINEL` for a SIG-own member so two
textually identical SIG declarations project to one schema), its `param_names` empty for a proper
type or naming the parameters of a constructor slot, and its `nonce` the generativity mint
(`None` for a SIG-body declaration, the per-call ascription module for an opaque mint). Module
identity is by `module.scope_id()`; signature identity is by schema *content* (`schema_digest`
alone — `WITH` pins are schema content by folding), so two textually identical declarations
name one type. Abstract-type identity is
by all four node fields — `param_names` feeds kind classification and `source` feeds member
substitution, so both are functional reads, not derivable payload. Satisfaction requires name
agreement, so two otherwise identical SIG declarations whose constructor slot names its parameter
differently are distinct types. The value channel carries a module as `KObject::Module`; the type
channel never names one directly, only through the self-sig that types it.
The type-position wildcard `KType::OfKind(KKind::Signature)` admits any
first-class signature value; the surface keyword `Signature` lowers to it in
[`KType::from_symbol`](../../src/machine/model/types/ktype_resolution.rs). The
`Module` surface keyword lowers to the **empty signature**
(`KType::EMPTY_SIGNATURE`, a `Signature` over the empty schema) — the lattice top
every module value satisfies — so an "any module" slot is signature-typed like
every other module slot rather than a kind wildcard.

The single `KType::Signature` variant serves both the constraint and the
value role, disambiguated by **position** rather than by variant. A
`Signature { .. }` *slot annotation* — `(PICK m :Ordered)` — matches a
*module value* on the value channel's Object arm whose self-sig structurally
satisfies `sig` (via [`KType::matches_value`](../../src/machine/model/types/ktype_predicates.rs)),
so `:Ordered` means "any module satisfying Ordered." A signature *value* —
`KType::Signature { .. }` in the `Type` arm, what `Ordered` evaluates to in
expression position — is matched by the `:Signature` (`OfKind(Signature)`)
wildcard. A slot typed `:Ordered` therefore never admits the signature
value itself, and `:Signature` never admits a satisfying module.

When a module satisfies two distinct signature slots at once, dispatch orders them by
**structural subtyping**, not by declaration order. The rule is uniform over every signature
type — `SIG`-declared, a module's self-sig, or the empty interface — since all three are the
same owned-schema shape: `:A` is more specific than
`:B` iff `A`'s schema is a *strict* `sig_subtype` of `B`'s — the forward
direction holds and the reverse fails. A `WITH` specialization refines its source by the same
rule (its folded manifest member satisfies forward and blocks reverse), and any non-empty
signature refines the empty interface. A slot whose signature requires strictly more
(`Wide` = `Base` plus an extra member) wins over the broader one. Two structurally-identical
signatures are mutually satisfying — forward and reverse both hold — so neither strictly
refines the other and dispatch surfaces `AmbiguousDispatch` rather than letting
declaration order silently pick a winner. The
[`is_more_specific_than`](../../src/machine/model/types/ktype_predicates.rs) walk
implements this, memoizing each direction under the `SigSatisfies` relation.

Module-typed bindings reuse the existing ascription operators:

```
LET m = (int_ord :! Ordered)   -- transparent: m.Carrier ≡ Number
LET m = (int_ord :| Ordered)   -- opaque:      m.Carrier is fresh
```

`:!` and `:|` are the typing primitives. There is no third
`LET m: Ordered = int_ord` form — it would express only the transparent
case and would be strictly less expressive than the operators that already
exist.

FN parameters and return types accept signature names directly. The
constrained-signature case (`(Ordered WITH {Carrier = Number})`)
uses the `WITH` builtin in
[functors.md § Type expressions and constraints](functors.md#type-expressions-and-constraints).

Signature-typed FN parameters plus first-class module values give
**dictionary-style polymorphism** directly: `(FN sort (ord :Ordered, xs :List) ...)` accepts any module satisfying `Ordered` as a single
passable value, and the dispatcher checks satisfaction at the call. The
witness module is passed by hand at every call site; the call-site
elision layer that drops the manual argument is described in
[implicits.md](implicits.md).

## Block-scoped opening (`USING … SCOPE`)

`(USING <module> SCOPE (exprs))` evaluates the block with the module's members in
scope as bare names and returns the value of the last expression. The receiver is
any module-valued expression, including a functor result opened inline. This is
a value-level namespace open in expression position — distinct from a file-level
import — so a region working against one instantiation writes `insert x s`
instead of `int_ord.insert x s`, stating the qualifier once.

The block runs in an **owned scope stacked inside a *transparent* window**, both
allocated by one door
([`Scope::open_module_window`](../../src/machine/core/scope/reach.rs)). The
window ([`Scope::child_transparent`](../../src/machine/core/scope.rs)) has the
call site as its `outer` and read-only bindings onto the module's child-scope
façade (`ScopeBindings::Borrowed`); the block's own scope is an ordinary owned
child of it, and is what the door returns — the window stays an internal middle
link, so a `Borrowed` table is never a write target. Reads therefore walk the
block's own binds, then the window, then the call-site chain, so module names win
over the call site inside the block; the resolver walk itself is unchanged. The
whole `Bindings` façade is borrowed, so every one of its tables
is surfaced: `data` (values), `functions` (dispatch overloads), `operators` (the
per-scope operator registry) and `types`. A module's type members therefore name
types by bare name inside the block — in sigil type expressions and in dispatch
slot types — exactly as its value members name values. This is how a
mutually-recursive group declared inside a module
([Module bodies announce their type declarations](#module-bodies-announce-their-type-declarations))
is constructed at its use site: the members bind bare inside the window. Opacity is preserved by
what the borrowed table *contains*, not by withholding a table: an opaque view's
child scope holds only the view's own members (the per-call abstract mints and the
signature's manifest members seeded at ascription, above), so an abstract member
surfaces as its `AbstractType` identity and the hidden representation is absent
from the window. The value side needs no window-specific machinery for the same
reason: the borrowed `data` table is the one the view was
[born holding coerced values in](#val-slot-reads-carry-the-abstract-member-identity),
so a bare-name read inside the block reports the same view-side type the qualified
ATTR read reports, and a function slot called bare inside the block is the coercion
wrapper. A transparent view's scope is seeded with the source's own bindings, so its
members read concretely. The `functions` buckets follow the same rule rather than
standing outside it: a view's buckets hold exactly the
[keyworded members](#keyworded-members) its signature declares, each at the overload
the source satisfies it with and wrapped where that overload crosses the barrier — so
a keyworded call inside the window reports the view's types, an undeclared key is not
in the window at all (the call walks out to the enclosing scope), and the two lanes of
one function agree. Because the registry rides the same façade, opening a module
that declares operators ([operators.md](../operators.md)) puts both their bodies
and their chaining mode in scope: a run inside the block reduces by the module's
own group.

Binds made inside the block are local to it and die when it closes; only the
tail expression's value escapes. Locality is structural rather than a teardown
action: a bind lands in the block's own scope at its plain statement index, and
no statement after the block reaches that scope on its ancestor walk. A block
statement sees exactly its earlier siblings' binds —
an intra-block forward reference stays a position error — and a bind or type
declaration whose name matches a surfaced member shadows the window from the
next statement on, ordinary inner-scope shadowing on all four binding channels
(values, `functions` buckets, types, operators). Nothing installs at the call
site, so statements after the block see none of it; a group of operations built
against the module escapes by value instead — define a module in the block and
return it from the tail statement, open the module inside a single function
definition, or write a functor from the module to a module of derived
operations. A module function dispatched inside the block resolves its own
internal names in the module's lexical scope: a `KFunction` carries its
definition scope and evaluates its body under it, so `USING` is purely a
lookup/dispatch surface, not a re-capture.

Both scopes are allocated in the **call-site region** — a transparent child is
same-region with its parent, and the block scope is an ordinary same-region child
of that — and the block is run as a deferred sub-dispatch whose result the
`USING` node lifts. Allocating in
the call-site region (rather than a per-call frame that drops at block end) is what
lets the tail value escape: the result — including a closure defined in the block,
which carries its captured scope — references values that live in the call-site
region, and the block's dead binding layer simply falls off the ambient chain
(scopes are `Drop`-free; nothing tears down). For a functor-result module whose child
scope lives in a per-call `CallFrame`, the opened module's value (carrying that
region's `Rc` per the
[per-call region protocol](../per-call-region/lifecycle.md#carriers)) is rooted in
the call-site region so the borrowed window survives both the block and any
closure that escapes it reading a surfaced member.

A bare `FN` registration writes only the `functions` dispatch bucket, never
`data`; only the combined `LET f = FN …` statement also writes `data`. The surfaced
window therefore carries captured values in `data` and the dispatch surface in
`functions`, cleanly separated rather than conflated.
