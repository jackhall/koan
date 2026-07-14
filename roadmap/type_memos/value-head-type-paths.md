# Value-head type paths

Modules bind value-side and type-position paths rooted at a module project through the value
channel; `KType` carries no module variant.

**Problem.** Module bindings live on the type channel: `MODULE`, `LET <Name> = <module
expr>`, and the module-argument parameter door all install `KType::Module` into
`bindings.types` ([`module_def.rs`](../../src/builtins/module_def.rs),
[`let_binding.rs`](../../src/builtins/let_binding.rs),
[`exec.rs`](../../src/machine/core/kfunction/exec.rs)), and reads re-surface the value
through `Scope::surface_type_hit`. A type-position path rooted at a module (`IntOrd.Type`
as an annotation head, a deferred `-> Er.Type` return) resolves through
[`KType::Module`](../../src/machine/model/types/ktype.rs) inside elaboration; the
overload-picker probe classifies a bare module name as `Carried::Type(KType::Module)`
([`resolve_name_part`](../../src/machine/execute/dispatch.rs)) and
[`body_identifier`](../../src/builtins/attr.rs) projects a signature-typed FUNCTOR
parameter's member through a type-side `KType::Module`. `AbstractType` carries `&Module`
(`AbstractSource::Module`) so further members can be projected off it. The type channel
hosts a module variant, and `KType` stays lifetime-entangled with `Module` through it.

**Acceptance criteria.**

- `MODULE`, `LET <Name> = <module expr>`, and the module-argument parameter door bind the
  module value-side (`bindings.data`), under the existing Type-token names; no binding door
  installs a module into `bindings.types`.
- Elaboration resolves a type path whose head is a module by projecting the type member off
  the value-channel module value; a *bare* module head in type position lowers to the
  module's self-sig (`Signature { SelfOf(m) }`), so `-> Er` with a module-valued parameter
  is a legal signature return and `x :IntOrd` is a structural slot admitting any module
  whose self-sig satisfies `IntOrd`'s.
- `KType` has no `Module` variant; `KKind::Module` is retired.
- `AbstractType`'s source is a plain `ScopeId` (the `AbstractSource` enum is deleted);
  further-member projection reads value-channel receivers, and projecting a member off a
  bare type-channel `AbstractType` is an error.
- The overload-picker probe classifies a bare module name as
  `Carried::Object(KObject::Module)`, and `body_identifier` has no type-side module
  projection arm.
- [Module naming flip](module-naming-flip.md) is scoped to naming policy only: its Problem
  and acceptance criteria assume value-side binding and name the Type-token resolver bridge
  this item leaves behind.

**Directions.**

- *Seam with the naming flip — decided.* This item owns all type-channel machinery; the
  naming flip owns surface policy (snake_case, `Sig`-suffix drop, rename churn). Modules
  keep their Type-token names here, resolved through explicitly-marked bridge arms in the
  resolver ladder (a Type-token part whose value-side hit is a module resolves to the
  Object arm); the naming flip deletes those arms when Type-token module names become
  errors.
- *Bare module head in type position — decided.* Lowers to `Signature { SelfOf(m) }`, not
  an error: slots and returns name signatures, and the self-sig is the module's type. Only
  `TypeIdentifier` elaboration gets the lowering; in type-language dispatch
  (`:(LIST OF IntOrd)`) the head resolves to a module value, which type slots refuse.
- *`AbstractSource` — decided.* Collapse to a bare `ScopeId`: both variants already key
  identity on `scope_id()`, and with no `&Module` the projection capability that
  distinguished them is gone. The variant becomes owned data (`to_static` succeeds,
  residence is trivial).
- *Phasing — decided.* Foundation phase (carries the risk; prototype against the functor
  deferred-return tests): the binding-door move, the resolver-ladder bridge, the self-sig
  lowering, and the data-channel stored-reach fallback the `type_identifier_memo` needs for
  `SelfOf` hits. Mechanical phases, each leaving the verify-koan slate green:
  compiler-guided deletion of `KType::Module` match arms, `KKind::Module` retirement, and
  the `AbstractSource` collapse. Miri audit slate after the foundation phase and after the
  collapse.

## Dependencies

**Requires:** none — its prerequisite (the `KObject::Module` value carrier) has shipped.

**Unblocks:**

- [Module naming flip](module-naming-flip.md) — snake_case heads in type position need
  value-head elaboration, and the flip deletes this item's Type-token bridge arms.
