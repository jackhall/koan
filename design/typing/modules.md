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
[Modular implicits](../../roadmap/module-system-5-modular-implicits.md).

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
