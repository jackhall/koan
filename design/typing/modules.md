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
SIG OrderedSig = ((LET Type = Number) (VAL compare :(Function (Type, Type) -> Number)))
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
`KType::UserType { kind: Module, scope_id, name }` per declared abstract
type. Two distinct opaque ascriptions of the same source module yield
distinct `scope_id`s and therefore distinct types that cannot be confused.
The carrier lives in
[`KType`](../../src/machine/model/types/ktype.rs); the operators are registered as
ordinary builtins in [`ascribe.rs`](../../src/builtins/ascribe.rs).

Opaque ascription is the type-abstraction primitive. It replaces the
newtype-with-private-fields pattern that a trait system would need.

## First-class modules

Modules are values: `KObject::KModule` flows through `LET`, ATTR, and
function calls like any other value. There is no separate pack/unpack form,
no `(module M)` construction syntax, and no `(val m)` projection. A module
named in expression position evaluates to its value, and `m.compare` is
ordinary attribute access.

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
constrained-signature case (`(SIG_WITH OrderedSig ((Type: Number)))`)
uses the `SIG_WITH` builtin in
[functors.md § Type expressions and constraints](functors.md#type-expressions-and-constraints).

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

The transparent scope is allocated in the **call-site arena**, and the block is
run as a deferred sub-dispatch whose result the `USING` node lifts. Allocating in
the call-site arena (rather than a per-call frame that drops at block end) is what
makes forwarding sound: a forwarded bind — or a function defined in the block and
forward-registered into the call site — references values and a captured scope
that all live in the call-site arena. For a functor-result module whose child
scope lives in a per-call [`CallArena`](../../src/machine/core/arena.rs), the
opened module's value (carrying that arena's `Rc`) is rooted in the call-site
arena so the borrowed window survives both the block and any closure that escapes
it reading a surfaced member.

A bare `FN` registration writes only the `functions` dispatch bucket, never
`data`; only the `LET f = (FN …)` capture form also writes `data`. The surfaced
window therefore carries captured values in `data` and the dispatch surface in
`functions`, cleanly separated rather than conflated.
