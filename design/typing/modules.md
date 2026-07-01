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
MODULE IntOrd = ((LET Type = Number) (LET compare = (FN ...)))
```

A **signature** (declared with `SIG`) is a module type — an interface
specifying what a structure must contain:

```
SIG OrderedSig = ((LET Type = Number) (VAL compare :(FN (x :Type, y :Type) -> Number)))
```

Module and signature names use the **Type-token** spelling: first character
ASCII-uppercase plus at least one lowercase character (`IntOrd`, `OrderedSig`,
`MakeSet`). Abstract types declared inside a signature use the same shape —
the convention is `Type` for the principal abstract type, with additional
abstract types named `Elt`, `Key`, `Val`, etc. when more than one is needed.
The token-class rule that distinguishes `MODULE` (keyword: ≥2 uppercase, no
lowercase) from `IntOrd` (Type token: uppercase-leading with at least one
lowercase) is described in [tokens.md](tokens.md).

SIG bodies accept two declarators. `LET <TypeName> = <expr>` declares an
abstract type slot (the binder name is Type-classified, so it lands on the
type-class binder path). `(VAL <name>: <TypeExpr>)` declares a value slot:
the canonical surface for naming an operation the signature requires, with
the slot's declared type recorded explicitly rather than inferred from an
example value. `VAL` is meaningful only inside a SIG body; outside it the
declarator is unbound. The lowercase-name `(LET name = <value>)` form is
rejected inside SIG bodies with a diagnostic directing to `VAL`. The implementation lives at
[`val_decl.rs`](../../src/builtins/val_decl.rs); ascription's
name-presence shape check ([`ascribe.rs`](../../src/builtins/ascribe.rs))
admits any module member that supplies the named slot regardless of how
the member was declared — full type-shape checking against the VAL slot's
declared type is owned by
[Modular implicits](../../roadmap/predicate_typing/modular-implicits.md).

Structures can be **ascribed** to signatures via two operators that differ
only by a whitespace gap in the visual rendering, expressing "you can see
through this":

```
LET IntOrdView     = (IntOrd :! OrderedSig)   -- transparent
LET IntOrdAbstract = (IntOrd :| OrderedSig)   -- opaque
```

*Transparent ascription* (`:!`) checks that the structure satisfies the
signature but leaves type definitions visible: `IntOrdView.Type` resolves to
`Number` just as `IntOrd.Type` does. *Opaque ascription* (`:|`) additionally
hides the representation: outside the ascription, `IntOrdAbstract.Type` is
**not** the same type as `Number`, even though that's its underlying
definition. Type checking forbids passing an `IntOrdAbstract.Type` value to
anything expecting a `Number` — the abstraction barrier is enforced.

Opaque ascription is **generative**: each application mints a fresh
`KType::AbstractType { source: Module(view), name }` per declared abstract
type, where `view` is the freshly allocated child module the ascription
introduces. `AbstractType`'s `source` is an
[`AbstractSource`](../../src/machine/model/types/ktype.rs) enum —
`Module(&'a Module<'a>)` for this per-call mint, `Sig(ScopeId)` for the
SIG-declaration-time member (below) — and manual `PartialEq` compares
`(source.scope_id(), name)`, so two opaque ascriptions of the same source
module yield distinct `scope_id`s and therefore distinct types that cannot be
confused, while two `KType::AbstractType` carriers minted from the same
ascription compare equal. The carrier lives in
[`KType`](../../src/machine/model/types/ktype.rs); the operators are registered as
ordinary builtins in [`ascribe.rs`](../../src/builtins/ascribe.rs).

### VAL-slot reads carry the abstract member identity

A SIG-local abstract-type binding stays *named* end to end, so a slot read
through an opaque view reports the abstract type rather than the underlying
representation. Three sites cooperate.

A SIG-local type binding (`LET Type = Number` inside a SIG body, the type-route
under `Scope::is_in_sig_body` in
[`let_binding.rs`](../../src/builtins/let_binding.rs)) binds the name-bearing
`KType::AbstractType { source: Sig(decl_scope_id), name }` rather than collapsing
to the underlying type, so a later `VAL zero :Type` records that `zero` *names*
the abstract member `Type`. A higher-kinded `LET Wrap = (TEMPLATE T)` is the
exception — it stays a `TypeConstructor` so ascription's per-call constructor mint
preserves the parameterization. Outer aliases and builtin annotations (`:Number`,
an outer `LET MyAlias = Number`) stay concrete.

Opaque ascription ([`ascribe.rs`](../../src/builtins/ascribe.rs)'s `body_opaque`),
after minting `type_members`, records on the new `Module` a `slot_type_tags` map
(VAL-slot name → per-call `AbstractType`) for each slot whose SIG-declared type is
a `Sig`-rooted member present in `type_members`. Transparent `:!` leaves the map
empty, so transparent reads stay concrete.

ATTR's `access_module_member`
([`attr.rs`](../../src/builtins/attr.rs)), on a value-side slot hit with a
`slot_type_tags` entry, re-tags the read into a
[`KObject::Wrapped`](../../src/machine/model/values/kobject.rs) carrier whose
`type_id` is the per-call abstract identity — the same `Wrapped` variant NEWTYPE
uses, distinguished by its `type_id`'s KType. So `(IntOrdView.zero)` reads as the
abstract `Type` (opaque), not the underlying `Number`, and a functor body
`(FN (GET_ZERO Er :WithZero) -> Er.Type = (Er.zero))` whose return
type is the per-call abstract member admits the slot read. The carrier and its
`type_id` are allocated in the *module's* region (declaration-stable), so the
`type_id` outlives any lift or deep-clone of the read value into a per-call functor
region.

Opaque ascription is the type-abstraction primitive. It replaces the
newtype-with-private-fields pattern that a trait system would need.

## First-class modules

The type language is first-class; modules and signatures live there. A
module value rides the value channel's `Type` arm as
[`KType::Module { module, frame }`](../../src/machine/model/types/ktype.rs)
and a signature value as `KType::Signature { sig, pinned_slots }` —
the same [`Carried::Type`](../../src/machine/model/values/carried.rs) arm that carries
`Number`, `Str`, and builtin type values, with the identity-bearing module/signature
variants living inside `KType` itself. A module value flows through `LET`, ATTR, and function
calls like any other value: there is no separate pack/unpack form, no
`(module M)` construction syntax, and no `(val m)` projection. A module
named in expression position evaluates to its value, and `m.compare` is
ordinary attribute access — ATTR projects through `KType::Module { module,
.. }` to reach `module.access_module_member(field)`. Member access is
**module-own**: one classified
[`Bindings::lookup_member`](../../src/machine/core/bindings.rs) reads the
module's own `data` then `types` and returns the value-or-type in a single pass
(the `data`/`types` cross-kind exclusion makes the result unambiguous), so a name
that isn't a declared member is a missing member — it does **not** fall through to
a builtin type or a lexically enclosing binding. `IntOrd.Type` therefore resolves
only when `IntOrd` declares a `Type` member (the `LET Type = …` convention),
never to the builtin `Type` meta-type. Signature member access
(`access_type_member` over `KType::Signature`) reads its decl scope the same way.

`MODULE` and `SIG` declarations are both **type-only**: finalize installs the
identity (`KType::Module { module, frame }` for MODULE, `KType::Signature {
sig, pinned_slots }` for SIG) into `bindings.types` via
[`Scope::register_type_upsert`](../../src/machine/core/scope.rs) and writes no
value-side carrier — `bindings.data` carries zero type carriers. `LET M2 = M1`
module aliases and `LET S2 = OrderedSig` signature aliases likewise route
through `register_type` against the type entry. Value-position references — a
module named as an ATTR receiver, a signature introspected by `:|` or `WITH`,
or either surfaced by `USING … SCOPE` — surface the
`KType::Module { .. } | KType::Signature { .. }` identity in the value channel's `Type` arm on
demand from the type entry via
[`resolve_type_leaf_carrier`](../../src/machine/execute/dispatch/resolve_type_identifier.rs);
ATTR's `body_type_lhs` routes its Type-classed receiver through that seam rather
than a raw `bindings.data` lookup.

`KType::Module` carries the live `&Module` pointer (plus the per-call
frame anchor for functor-built modules); `KType::Signature { sig, pinned_slots }`
carries the region-pinned `&Signature` plus any `WITH` abstract-type
pins; `KType::AbstractType { source, name }` carries an abstract-type member —
either a SIG-declared member (`source: Sig(scope_id)`) or the per-call mint of an
opaquely-ascribed module (`source: Module(view)`). Module identity is by
`module.scope_id()`; signature identity by `sig.sig_id()` + `pinned_slots`;
abstract-type identity by `(source.scope_id(), name)`. The
type-position wildcards `KType::OfKind(KKind::Module)` and `KType::OfKind(KKind::Signature)`
admit any first-class module or signature value — the surface keywords
`Module` and `Signature` lower to them in
[`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs).

The single `KType::Signature` variant serves both the constraint and the
value role, disambiguated by **position** rather than by variant. A
`Signature { .. }` *slot annotation* — `(PICK m :OrderedSig)` — matches a
*module* whose `compatible_sigs` records `sig.sig_id()`, so `:OrderedSig`
means "any module satisfying OrderedSig." A signature *value* —
`KType::Signature { .. }` in the `Type` arm, what `OrderedSig` evaluates to in
expression position — is matched by the `:Signature` (`OfKind(Signature)`)
wildcard. A slot typed `:OrderedSig` therefore never admits the signature
value itself, and `:Signature` never admits a satisfying module.

Module-typed bindings reuse the existing ascription operators:

```
LET m = (IntOrd :! OrderedSig)   -- transparent: m.Type ≡ Number
LET m = (IntOrd :| OrderedSig)   -- opaque:      m.Type is fresh
```

`:!` and `:|` are the typing primitives. There is no third
`LET m: OrderedSig = IntOrd` form — it would express only the transparent
case and would be strictly less expressive than the operators that already
exist.

FN parameters and return types accept signature names directly. The
constrained-signature case (`(OrderedSig WITH {Type = Number})`)
uses the `WITH` builtin in
[functors.md § Type expressions and constraints](functors.md#type-expressions-and-constraints).

Signature-typed FN parameters plus first-class module values give
**dictionary-style polymorphism** directly: `(FN sort (ord :OrderedSig, xs :List) ...)` accepts any module satisfying `OrderedSig` as a single
passable value, and the dispatcher checks satisfaction at the call. The
witness module is passed by hand at every call site; the call-site
elision layer that drops the manual argument is described in
[implicits.md](implicits.md).

## Block-scoped opening (`USING … SCOPE`)

`(USING Module SCOPE (exprs))` evaluates the block with `Module`'s members in
scope as bare names and returns the value of the last expression. `Module` is
any module-valued expression, including a functor result opened inline. This is
a value-level namespace open in expression position — distinct from a file-level
import — so a region working against one instantiation writes `insert x s`
instead of `IntOrd.insert x s`, stating the qualifier once.

The block runs in a single *transparent* scope
([`Scope::child_transparent`](../../src/machine/core/scope.rs)) whose `outer` is
the call site and whose bindings are a read-only window onto the module's
child-scope façade (`ScopeBindings::Borrowed`). Reads consult the window first,
then the call-site chain, so module names win inside the block; the resolver walk
is unchanged. Only the module's `data` (values) and `functions` (dispatch
overloads) are surfaced — the whole `Bindings` façade is borrowed, while a
module's abstract type ascriptions live in `Module::type_members`, *not* in
`Bindings`, so opacity is preserved inside the block.

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
