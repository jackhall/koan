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
MODULE int_ord = ((LET Type = Number) (LET compare = (FN ...)))
```

A **signature** (declared with `SIG`) is a module type — an interface
specifying what a structure must contain:

```
SIG Ordered = ((TYPE Type) (VAL compare :(FN (x :Type, y :Type) -> Number)))
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
too — the convention is `Type` for the principal abstract type, with additional
abstract types named `Elt`, `Key`, `Val`, etc. when more than one is needed. The
token-class rule that distinguishes `MODULE` (keyword: ≥2 uppercase, no lowercase)
from `Ordered` (Type token) and `int_ord` (Identifier) is described in
[tokens.md](tokens.md).

SIG bodies accept three declarators, split by what a satisfying module must
supply:

- `TYPE <TypeName>` declares an **abstract** type member — a witness-less slot
  the module supplies at any concrete type. `TYPE (<Param> AS <Name>)` is the
  higher-kinded form (see [Higher-kinded type slots](functors.md#higher-kinded-type-slots)).
  `TYPE` is meaningful only inside a SIG body; the implementation lives at
  [`type_decl.rs`](../../src/builtins/type_decl.rs).
- `LET <TypeName> = <expr>` declares a **manifest** type member — a slot fixed
  to the RHS type. A satisfying module's member must equal it. Inside a SIG body
  the invariant is `=`-iff-manifest: a Type-class `LET` binds the concrete RHS
  (no abstract re-tag), and abstract members use `TYPE`, which has no RHS.
- `(VAL <name> :<TypeExpr>)` declares a value slot: the canonical surface for
  naming an operation the signature requires, with the slot's declared type
  recorded explicitly rather than inferred from an example value.

A SIG body's declarators all write **one table**: the decl scope's `types` map records each
`TYPE <Name>` abstract member under its Type-token name *and* each `VAL <name> :<Type>` slot's
declared type under the slot's **value** name.
[`SigSchema`](../../src/machine/model/types/sig_schema.rs) splits it back apart — by
representation for abstract-vs-manifest, by token class for type members vs value slots. This
**slot table** is a schema, not a binding universe, and it is the sole exemption from the
token-class partition that otherwise keeps value tokens out of the type map
([elaboration.md § Binding-map partition](elaboration.md#binding-map-partition)): a SIG body's
`Bindings` is built by `Bindings::new_slot_table()`.

`VAL` and `TYPE` are meaningful only inside a SIG body; outside it the
declarator is unbound. The lowercase-name `(LET name = <value>)` form is
rejected inside SIG bodies with a diagnostic directing to `VAL`. The implementation lives at
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
the signature but leaves type definitions visible: `int_ord_view.Type` resolves to
`Number` just as `int_ord.Type` does. *Opaque ascription* (`:|`) additionally
hides the representation: outside the ascription, `int_ord_abstract.Type` is
**not** the same type as `Number`, even though that's its underlying
definition. Type checking forbids passing an `int_ord_abstract.Type` value to
anything expecting a `Number` — the abstraction barrier is enforced.

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
representation. Three sites cooperate.

A `TYPE Type` declaration ([`type_decl.rs`](../../src/builtins/type_decl.rs)) binds
the name-bearing `KType::AbstractType { source: <decl scope id>, name }`, so a
later `VAL zero :Type` records that `zero` *names* the abstract member `Type`. The
higher-kinded `TYPE (Type AS Wrap)` binds a sentinel `TypeConstructor` so
ascription's per-call constructor mint preserves the parameterization. A manifest
`LET Tag = Number` binds the concrete `Number` — it carries no abstract identity,
and a `VAL x :Tag` slot reads through concretely. Classification is by
*representation*, not name class:
[`sig_schema.rs`](../../src/machine/model/types/sig_schema.rs)'s `is_abstract_sig_member`
reads the member's `KType` shape (an `AbstractType` or a sentinel constructor
is abstract; everything else is manifest). Outer aliases and builtin annotations
(`:Number`, an outer `LET MyAlias = Number`) stay concrete.

Opaque ascription ([`ascribe.rs`](../../src/builtins/ascribe.rs)'s `body_opaque`)
mints a per-call `AbstractType` into `type_members` for each abstract member and
mirrors each manifest member's fixed `KType` in concretely (the view scope carries
no type entries of its own), then records on the new `Module` a `slot_type_tags` map
(VAL-slot name → per-call `AbstractType`) for each slot whose SIG-declared type is
an abstract member sourced at the SIG's decl scope. Transparent `:!` leaves the map empty, so
transparent reads stay concrete.

**Satisfaction and `WITH`.** Satisfaction is a **signature-subtyping** check
([`sig_schema.rs`](../../src/machine/model/types/sig_schema.rs)). Every module carries a
principal **self-sig** — a [`SigSchema`](../../src/machine/model/types/sig_schema.rs) of its
abstract members (always none — `TYPE` is SIG-body-only), manifest type members, and
value-slot types — derived once at creation and sealed immutable (`Module::seal_self_sig`;
a bare construction derives it lazily via `raw_self_sig`). A signature likewise projects to a
`SigSchema` (`SigSchema::of_sig`, which folds any `WITH` pins in). Ascription (`check_satisfies`,
run for both `:|` and `:!`) holds iff `module.self_sig <: sig-schema` under `sig_subtype`:
`Sub <: Super` iff `Sub` supplies every member `Super` names (width — extra `Sub` members are
ignored), with each manifest member *equal*, each abstract member present at the matching
kind/arity (a first-order slot needs a proper type or first-order member; a higher-kinded
`TYPE (Type AS Wrap)` slot needs a constructor of the same arity), and each value slot
covariantly compatible — the module's member type must be `satisfied_by`-admissible for the
slot's declared type, after the slot's references to `Super`'s abstract members are substituted
with `Sub`'s bindings for them. Each ascription view seals its own self-sig recording those
substituted slot types, so a view structurally satisfies its own signature. The result is
memoized per `sig_id` on the module (a pure cache — types are immutable).

Dispatch matching of a `:Sig` slot runs the same structural check ascription asserts
([`Module::structurally_satisfies`](../../src/machine/model/values/module.rs), memoized per
`sig_id`), plus, for a `WITH`-pinned slot, `satisfies_pins` — every pin naming a self-sig
manifest member fixed equal
([`ktype_predicates.rs`](../../src/machine/model/types/ktype_predicates.rs)). Ascription is
assertion plus view construction, never an admission gate: an unascribed module whose self-sig
satisfies a signature is admitted by that signature's slot directly.
`WITH` pins abstract slots; a pin naming a manifest member is normalized away when it equals the
fixed type (leaving signature identity unchanged) and is a type error when it differs
([`type_ops/with.rs`](../../src/builtins/type_ops/with.rs)).

ATTR's `access_module_member`
([`attr.rs`](../../src/builtins/attr.rs)), on a value-side slot hit with a
`slot_type_tags` entry, re-tags the read into a
[`KObject::Wrapped`](../../src/machine/model/values/kobject.rs) carrier whose
`type_id` is the per-call abstract identity — the same `Wrapped` variant NEWTYPE
uses, distinguished by its `type_id`'s KType. So `(int_ord_view.zero)` reads as the
abstract `Type` (opaque), not the underlying `Number`, and a functor body
`(FN (GET_ZERO er :WithZero) -> er.Type = (er.zero))` whose return
type is the per-call abstract member admits the slot read. The carrier and its
`type_id` are allocated in the *module's* region (declaration-stable), so the
`type_id` outlives any lift or deep-clone of the read value into a per-call functor
region.

Opaque ascription is the type-abstraction primitive. It replaces the
newtype-with-private-fields pattern that a trait system would need.

## First-class modules

The type language is first-class; modules and signatures live there. A
module value rides the value channel's Object arm as
[`KObject::Module`](../../src/machine/model/values/kobject.rs), typed by its
principal signature — `ktype()` reports
`KType::Signature { sig: SigSource::SelfOf(m), .. }`, so dispatch trusts the
carried self-sig. A signature value rides the
[`Carried::Type`](../../src/machine/model/values/carried.rs) arm as
`KType::Signature { sig, pinned_slots }` — the same arm that carries `Number`,
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
a builtin type or a lexically enclosing binding. `int_ord.Type` therefore resolves
only when `int_ord` declares a `Type` member (the `LET Type = …` convention),
never to the builtin `Type` meta-type. Signature member access
(`access_type_member` over `KType::Signature`) reads its decl scope the same way.

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
`KType::Signature { sig, pinned_slots }` into `bindings.types` via
[`Scope::register_type_upsert`](../../src/machine/core/scope.rs), and a `LET S2 =
Ordered` signature alias routes through `register_type` against that entry. A
signature identity rides the `Type` arm, surfaced from the type entry on demand by
[`Scope::resolve_type_identifier`](../../src/machine/execute/dispatch/resolve_type_identifier.rs).

A module name is an Identifier token, so the resolver ladder reads it on the
**value** channel like any other value name: no ladder arm is keyed to "a Type token
that turns out to name a module", and a module's value write clears no Type-kind
placeholder. ATTR's `body_module` reads its module receiver off the Object arm, so
`int_ord.Type` and `er.Carrier` alike project off the module *value*; there is no
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
(`KType::Signature { sig: SigSource::SelfOf(m), pinned_slots: [] }`), which is how a
module reaches a slot, a return, or a `LET` type alias. Its `value` slot is
`KType::Any`, which admits both channels, so a *type* argument reaches the body and
is refused there with a diagnostic rather than falling through dispatch as a miss;
an empty, unstamped container is refused too (no knowable element type). The result
is built at the fold brand from the argument's own carrier, so the type it produces
borrows exactly the region the value lives in — a module minted in a function's
per-call region included.

Three consequences:

- `-> :(TYPE OF er)`, where `er` is a module-valued parameter, is a deferred return
  meaning "returns a module satisfying `er`'s interface". The per-call contract is
  the argument module's self-sig, homed into the captured-scope region against the
  **delivered carrier** of the resolved return type
  ([`home_delivered_return_type`](../../src/machine/core/kfunction/exec.rs)), whose
  reach names every region that type borrows. The argument module need not live in
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
widening: FN carries a value-named-return overload whose only job is to name the
`:(TYPE OF er)` respelling, and a return-type elaboration miss is surfaced instead of
falling back to `Any`. All of this is pinned by
[`module_head_in_type_position`](../../src/builtins/fn_def/tests/functor/module_head_in_type_position.rs)
and [`type_of/tests.rs`](../../src/builtins/type_ops/type_of/tests.rs).

Each [`Module`](../../src/machine/model/values/module.rs) seals a principal self-sig
([`SigSchema`](../../src/machine/model/types/sig_schema.rs)) at creation — the immutable
structural type the satisfaction relation reads (see §"Satisfaction and `WITH`").
`KType::Signature { sig, pinned_slots }`
carries a [`SigSource`](../../src/machine/model/types/ktype.rs) — the three
points of the module lattice: `Declared(&Signature)` for a `SIG` declaration,
`SelfOf(&Module)` for a module value's principal signature, and `Empty` for the
empty signature — plus any `WITH` abstract-type
pins; `KType::AbstractType { source, name }` carries an abstract-type member, its
`source` naming either the SIG decl scope (a declared member) or the per-call
ascription module (an opaque mint). Module identity is by `module.scope_id()` — the
key both `SelfOf` and an `AbstractType` minted off that module digest on; signature
identity by `sig.sig_id()` + `pinned_slots`; abstract-type identity by
`(source, name)`. The value channel carries a module as `KObject::Module`; the type
channel never names one directly, only through the self-sig that types it.
The type-position wildcard `KType::OfKind(KKind::Signature)` admits any
first-class signature value; the surface keyword `Signature` lowers to it in
[`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs). The
`Module` surface keyword lowers to the **empty signature**
(`KType::empty_signature()`, `Signature { SigSource::Empty }`) — the lattice top
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

When a module satisfies two distinct SIG slots at once, dispatch orders them by
**structural subtyping**, not by declaration order: `:A` is more specific than
`:B` iff `of_sig(A)` is a *strict* `sig_subtype` of `of_sig(B)` — the forward
direction holds and the reverse fails (`WITH` pins fold into `of_sig` on both
sides). A slot whose signature requires strictly more (`Wide` = `Base` plus an
extra member) wins over the broader one. Two structurally-identical distinct SIGs
are mutually satisfying — forward and reverse both hold — so neither strictly
refines the other and dispatch surfaces `AmbiguousDispatch` rather than letting
declaration order silently pick a winner. The
[`is_more_specific_than`](../../src/machine/model/types/ktype_predicates.rs) walk
implements this, memoizing each direction under the `SigSatisfies` relation.

Module-typed bindings reuse the existing ascription operators:

```
LET m = (int_ord :! Ordered)   -- transparent: m.Type ≡ Number
LET m = (int_ord :| Ordered)   -- opaque:      m.Type is fresh
```

`:!` and `:|` are the typing primitives. There is no third
`LET m: Ordered = int_ord` form — it would express only the transparent
case and would be strictly less expressive than the operators that already
exist.

FN parameters and return types accept signature names directly. The
constrained-signature case (`(Ordered WITH {Type = Number})`)
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

The block runs in a single *transparent* scope
([`Scope::child_transparent`](../../src/machine/core/scope.rs)) whose `outer` is
the call site and whose bindings are a read-only window onto the module's
child-scope façade (`ScopeBindings::Borrowed`). Reads consult the window first,
then the call-site chain, so module names win inside the block; the resolver walk
is unchanged. Only the module's `data` (values), `functions` (dispatch
overloads), and `operators` (the per-scope operator registry) are surfaced — the
whole `Bindings` façade is borrowed, while a module's abstract type ascriptions
live in `Module::type_members`, *not* in `Bindings`, so opacity is preserved
inside the block. Because the registry rides the same façade, opening a module
that declares operators ([operators.md](../operators.md)) puts both their bodies
and their chaining mode in scope: a run inside the block reduces by the module's
own group.

Binds made inside the block forward to the call site and persist after it; a bind
whose name collides with a surfaced member is rejected
([`Scope::bind_value`](../../src/machine/core/scope.rs)'s borrowed-window arm), so
a forwarded bind can never be silently shadowed by the window. Forwarding outward
is safe because the block is unconditional — unlike `TRY`/`MATCH` branches it
always runs, so there is no divergent-binding hazard. A module function dispatched
inside the block resolves its own internal names in the module's lexical scope:
a `KFunction` carries its definition scope and evaluates its body under it, so
`USING` is purely a lookup/dispatch surface, not a re-capture.

The transparent scope is allocated in the **call-site region**, and the block is
run as a deferred sub-dispatch whose result the `USING` node lifts. Allocating in
the call-site region (rather than a per-call frame that drops at block end) is what
makes forwarding sound: a forwarded bind — or a function defined in the block and
forward-registered into the call site — references values and a captured scope
that all live in the call-site region. For a functor-result module whose child
scope lives in a per-call `CallFrame`, the opened module's value (carrying that
region's `Rc` per the
[per-call region protocol](../per-call-region/lifecycle.md#carriers)) is rooted in
the call-site region so the borrowed window survives both the block and any
closure that escapes it reading a surfaced member.

A bare `FN` registration writes only the `functions` dispatch bucket, never
`data`; only the `LET f = (FN …)` capture form also writes `data`. The surfaced
window therefore carries captured values in `data` and the dispatch surface in
`functions`, cleanly separated rather than conflated.
