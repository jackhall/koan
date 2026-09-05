# Expression shapes are their own kind of function

An expression shape — a keyworded, positional definition reached by dispatch — is declared with
`EXPR` and carries a type that records its keyword and argument-position sequence, so it is no
longer spelled or typed as if it were a lambda.

**Problem.** Koan has two kinds of function and one word for both. A **lambda** is an anonymous
callable reached by name, taking a record of named arguments and returning a value; its type is
written `:(FN :{x :Number} -> Bool)`. An **expression shape** is a keyworded definition —
`FN (PURE x :Carrier) -> Carrier = (x)` — reached by dispatch on the keyword/argument sequence that
forms its bucket key, never by name. The second is the first plus a layer: a fixed sequence of
keywords interleaved with argument positions.

Three consequences of the conflation, all live:

- **The type drops the layer.** A signature's keyworded member
  ([design/typing/modules.md](../../design/typing/modules.md)) has as its type an interned
  `KFunction` node — a record of named argument types plus a return
  ([sig_schema.rs](../../src/machine/model/types/sig_schema.rs)). A record canonicalizes its field
  order, and the keywords are nowhere in it. So `(PURE x :Number) -> Number` and
  `(HIDE x :Number) -> Number` are one type, and satisfaction, digests, `TYPE OF`, rendering and
  `fn_type_specificity` all compare shapes that differ on the surface as if they were the same. The
  untyped key is carried *beside* the type, as the map's key, rather than being part of it — and
  the record's canonical order also erases the argument *positions*, so two overloads that differ
  only by parameter order share one type while carrying distinct dispatch tokens.
- **The declaration surface is `FN` twice over.** The bodyless declarator shares its dispatch bucket
  key `[FN, Slot, ->, Slot]` with the function-*type* expression `FN :{…} -> <Ret>`
  ([parameterized_types.rs](../../src/builtins/parameterized_types.rs)), separated only by a
  lazy-slot staging entry that captures the head raw
  ([lazy_slots.rs](../../src/machine/model/lazy_slots.rs)). The two forms mean unrelated things and
  the reader has to know which context they are in to tell them apart. The diagnostic shows the
  cost: a malformed function type such as `:(FN (x :Number) -> Bool)` reaches the declarator and is
  advised to *"write `FN (<head>) -> <Return> = (<body>)` to define a function"*, which is wrong
  advice inside a type sigil.
- **A shape cannot be named as a type at all.** There is no spelling for "the type of the
  expression shape `(PURE _ :Number) -> Number`". A `VAL` slot can hold a lambda; only a SIG's
  keyworded channel can talk about a shape, and only by declaring one.
- **A member cannot quantify over a type.** A declared member names concrete types and nothing
  else, so an operation that holds *for every* element type has no spelling. The declarator
  refuses a return type naming one of its own parameters
  ([design/typing/modules.md](../../design/typing/modules.md)) — a definition resolves such a
  return per call, and a declaration has no call — and a `VAL` slot's `KFunction` is closed
  before it is stored. So `(PURE x :Number) -> :(Number AS Wrap)` is declarable and
  "`PURE` at every element type" is not, and a signature wanting the latter must be written once
  per element type, with each writing minting an unrelated `Wrap`. This is what blocks the
  `Monad` signature of [design/effects.md](../../design/effects.md), whose `bind` takes one
  element type in and hands a different one back under a single wrapper.

**Acceptance criteria.**

- A SIG declares a keyworded member with `EXPR` —
  `SIG S = ((TYPE Carrier) (EXPR (PURE x :Carrier) -> Carrier))` — and the bodyless `FN` head no
  longer declares one. `FN` in a type position is unambiguously a lambda type again, and the
  bucket-key collision, its lazy-slot staging entry, and its misdirected diagnostic are all gone.
- Definition sites respell too: a bucket-only definition is written
  `EXPR (PURE x :Carrier) -> Carrier = (x)`, and the dual-binding statement — one callable
  reaching both the value name and the dispatch bucket — is written
  `LET pure = FN EXPR (PURE x :Carrier) -> Carrier = (x)`, naming both kinds it binds. The
  record-schema lambda forms keep `FN`. The retired keyworded `FN` spellings are refused with
  diagnostics naming the respelling.
- An expression shape's type is the interleaved element sequence — keywords and positional
  argument types, in order — plus the return type. Argument names are binder-side only: they
  appear where a body needs them and are absent from the type, so two definitions differing only
  in parameter names project one shape type. Two shapes differing in keyword placement or
  argument order are different types — distinct digests, distinct rendering, distinct `TYPE OF`
  on the schemas that carry them — and a shape type is never equal to, never satisfies, and is
  never satisfied by a lambda type.
- Every consumer of a keyworded member's type reads the shape — key included — from the type
  rather than from the map key it is stored under: satisfaction, `join_schemas`, `fold_pins`,
  specificity, rendering, and the view's install path.
- A schema's keyworded members are a set of shape types, not a map keyed by `UntypedKey`: the key
  is recoverable from each member's own type, so `SigSchema` stores no second copy of it and
  `KeywordedMembers` is gone. The scope-side dispatch bucket table keeps its `UntypedKey` — that
  key indexes candidates for a call, which is a different job.
- Specificity over two shapes pairs their argument positions positionally, through the one
  `specificity_over` lattice fold. `fn_type_specificity` — the by-name pairing that exists solely
  to rank keyworded candidates in `select_keyworded_satisfier` — is deleted, leaving one pairing
  rule for shapes and none that reads argument names.
- A shape type is spellable in a type position — `:(EXPR (PURE _ :Number) -> Number)` — so the
  same type can be written down outside a SIG body, and a slot that expects one refuses a lambda
  while admitting a callable whose registered shape satisfies it.
- A shape **quantifies** over type parameters it binds: `EXPR (PURE Elt :Type x :Elt) -> :(Elt AS Wrap)`
  declares one operation holding at every `Elt`, and the quantified names are readable by every
  later element type and by the return. The quantifier list is part of the shape type — two
  shapes quantifying over different arities are different types, and it renders and digests with
  the rest of the shape — as against argument names, which are binder-side only. A shape
  quantifying over nothing is the ordinary case and carries an empty list.
- Satisfaction of a quantified shape solves its parameters **once, against the candidate's own
  overloads**, and admits only a module supplying an implementation that holds at every
  instantiation — never one implementation per instantiation. A candidate satisfying the shape
  at some element types and not others is a failure naming the parameter that failed to solve.
- `SIG Monad = ((TYPE (Type AS Wrap)) (EXPR (PURE …)) (EXPR (BIND …)))` — the signature of
  [design/effects.md](../../design/effects.md) — declares, and a module ascribes it once for
  every element type.
- Satisfaction and the ascription view select over **one** representation of a shape and reach the
  relation through **one** entrance: both call the same selection function on the module's
  self-sig overloads under the same substitution, and the view then locates the callable by
  shape-type equality — total, because the shape type carries everything the dispatch table
  distinguishes overloads by. The `debug_assert!` in `install_keyworded_surface`
  ([registry.rs](../../src/machine/core/scope/registry.rs)) guarding a disagreement between the two
  goes away because the disagreement is unrepresentable, not because it is checked.
- The abstract-member substitution has one entrance too: the view install runs the same
  substituting comparison the check runs (`slot_satisfied_by`, given the binder and the source's
  bindings), and the pre-substitute-then-plain-rule route is deleted.

**Directions.**

- *Where the shape lives in the type — decided.* A distinct `ExpressionShape` type node — a
  lifetime-free boxed slice of elements, each a keyword or a positional argument type, plus the
  return — so "a lambda is not a shape" is a variant fact and the element sequence carries the
  argument order the params record canonicalizes away. `KFunction` is untouched and keeps lambdas.
- *Argument names and pairing — decided.* The shape type is nameless in its *arguments*;
  satisfaction, specificity and the join pair argument positions, mirroring dispatch, which never
  sees names. No shape-to-lambda coercion exists anywhere to demand names in the type: the
  dual-binding form's value half is an ordinary lambda-typed binding, so `VAL` slots keep working
  unchanged. A quantified parameter is the exception and is not an argument name — see below.
- *Quantified parameters ride the shape — decided.* `ExpressionShape` carries a quantifier list
  beside its element sequence: the type parameters the shape binds, whose names the later
  elements and the return read. They are load-bearing in the type, where argument names are not,
  because an element type dereferences them; so they feed the digest, render, and pair
  positionally in specificity and the join. Two shapes alpha-equivalent under a renaming of their
  quantified parameters are one type — the names are a binding, not identity — which keeps
  "argument names are absent from the type" and "quantified names are present" from colliding.
- *Solving a quantified parameter — decided.* Reuse the shipped deferred-return shadow
  ([`TypeNode::DeferredReturn`](../../src/machine/model/types/ktype.rs)), which already stands for
  "a type this position names but has not resolved," moved from a definition's per-call
  elaboration to a declaration's satisfaction-time one. Satisfaction walks the shape and the
  candidate overload in step, binding each quantified name to what the candidate has at that
  position and requiring every later occurrence to agree. This is the parameter-side mirror
  [stage 5](../predicate_typing/modular-implicits.md) wants for implicit-functor resolution, but
  it needs nothing from that stage: the shadow node and the structural walk are this item's.
- *Where the selection is carried — decided.* Nowhere: the verdict stays boolean and the view
  re-runs the one shared selection function on the same immutable inputs, then resolves the
  callable by shape-type equality. Deterministic, so provably the same pick, with no payload
  threaded through the satisfaction memo.
- *One `EXPR` surface — decided.* One builtin: `EXPR (…) -> Ret` evaluates to the shape type
  value wherever it stands (under a type sigil or bare, matching how `FN :{…} -> Ret` behaves),
  and a statement directly in a SIG body declares the member instead. The type-sigil stamp keeps
  a sigiled use inside a SIG body on the type-value path.
- *`EXPR` at the definition site — decided.* Definitions respell in this item: bare keyworded
  definitions to `EXPR … = (body)`, the combined statement to `LET <name> = FN EXPR … = (body)`.
- *Operator members — decided per prerequisite.* An `OP`'s overloads are keyworded members
  already, so they take shape types with no operator-specific handling; the declaration surface
  and the registry half are settled first by
  [SIG operator members](sig-operator-members.md).

## Dependencies

The keyworded surface this re-spells and re-types is shipped —
[design/typing/modules.md](../../design/typing/modules.md) covers the declaration form, an
overload's identity, satisfaction as a dispatch-mirrored most-specific pick, and the ascription
barrier's treatment of it.

**Requires:**

- [SIG operator members](sig-operator-members.md) — the shape representation must cover operator
  members, so their surface is settled first.

**Unblocks:**

- [Monadic side effects](../foundation/monadic-side-effects.md) — the `Monad` signature's `pure`
  and `bind` are quantified members.
