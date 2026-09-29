# Modules

Module programs: the ascription and member-reading forms, running.

**Problem.** The [module layer](../../src/knot/module/README.md) holds the module
value, its self-signature, and the doors `:|`, `:!` and `USING … SCOPE` name,
each exercised over an activation whose members a test binds by hand. Nothing
evaluates them. An ascription is an expression, a member read is an attribute
expression shape, and a name a `USING` surfaces resolves to a coordinate a running reader
has to redeem, and [dispatch](../../src/dispatch/README.md) answers each with an
error value, since nothing evaluates it. No module program runs on the rewritten
stack, and the old runtime's [`Module`](../../src/machine/model/values/module.rs)
is the module surface `machine` still owns. The layer's signatures hide a type
through an abstract `TYPE` member, and each evaluation of `:|` mints a fresh
nonce for it, so two opaque views of one module never share a carrier.

**Acceptance criteria.**

- An ascription expression evaluates: `:|` and `:!` run the view door where they
  are written and bind the view module it hands back.
- A member read evaluates: `m.f`, and a name surfaced by `USING … SCOPE`, read
  the [member run](../../src/knot/module/README.md#layout-order) the module holds
  where the reader runs, and a member that is itself a knot member crosses to
  the reader priced as its knot.
- A registration a `USING … SCOPE` operand surfaces is a candidate for a
  keyworded use in its body, and one as specific as a registration visible
  from outside the body makes the call an ambiguity error.
- A coerced function member runs: calling the
  [barrier](../../src/knot/module/README.md#members-are-born-coerced) an opaque view
  holds rewrites each argument from the view's
  types to its root implementation's, runs the underlying function, and rewrites
  the result to the view's types where the call returns.
- A signature quantifies at its head: `SIG Counter FOR ALL #{Carrier: Any} = #[…]`
  elaborates, `Counter WITH {Carrier = Number}` is its application, and a `TYPE`
  member in a signature is refused where its shape is built.
- `ints :! Counter` solves `Carrier` against `ints`'s member types, as a call
  solves a `FOR ALL` against its arguments, and the view shows the solution;
  under `ints :| Counter` the view's `Carrier` is an unbounded carrier that no
  code outside the view constructs or matches.
- Two evaluations of `ints :| Counter`, and `(ints :| Counter) :| Counter`,
  yield views whose carriers are one type, so a value read from one passes to
  the other's members; `ints :| Counter` and `strs :| Counter`, or `ints` under
  two signatures, yield carriers that do not unify.
- `(ints :| Counter) :| (Counter WITH {Carrier = Number})` is refused, and
  `(ints :| Counter) :! Counter` shows the carrier, never `Number`.
- A head quantifier's bound gates satisfaction only: a signature quantified over
  `#{Carrier: Number}` refuses a module whose carrier solves to `Str`, and a
  `:|` view's carrier is a non-match at a `:Number` slot.
- `MODULE doubling :Doubler = (…)` checks the module against `Doubler` when the
  statement runs and binds it unnarrowed, so a member `Doubler` does not declare
  stays readable; a module that fails the check is an error at the statement,
  and `GROUP <name> :<Sig> FOLD LEFT = (…)` checks a group alike.
- A definition in a body annotated with a signature takes its ranking from the
  signature's member head at its key, so
  `MODULE m :Mover = (EXPR #(MOVE p :Piece TO s :Square) -> Board = #(…))`
  satisfies a `Mover` declaring `EXPR #(MOVE 2 :Piece TO 1 :Square) -> Board`
  with no bucket declaration of its own.
- A builtin native reads a sealed builtin value its overload admitted through
  the seal, as [`unsealed`](../../src/values/admission.rs) reads one for
  equality: a sealed `Number` a view's member hands to `+`, or to any native
  whose slot admits it, computes as the number, and no member of a view over a
  builtin type reaches a native's invariant check.
- The old runtime's tutorial programs that use modules run on the rewritten
  stack and print the same output.

**Directions.**

- *Where the doors live — decided.* In the
  [module layer](../../src/knot/module/README.md). This item evaluates them and adds
  no door of its own.
- *Parameterized signatures — decided.* A signature quantifies at its head,
  `SIG Ordered FOR ALL #{Carrier: Bound} = #[…]`, and
  `Ordered WITH {Carrier = Number}` is an application. A signature declares no
  `TYPE` member, so the abstract member, its nonce and the signature meet over
  abstract members go. Ascription solves the head quantifier against the
  module's member types with the unifier a call solves a `FOR ALL` with, and
  `ints :! (Ordered WITH {Carrier = …})` stays writable. Under `:!` the view's
  `Carrier` is the solution; under `:|` it is an unbounded carrier, so no slot
  outside the view admits it and the value is hidden. The bound gates
  satisfaction and reveals nothing under `:|`. The
  [barrier](../../src/knot/module/README.md#members-are-born-coerced) stays: a
  scalar carries no type of its own, so hiding a `0` wraps it, while a
  container is retyped with no wrapper. Existential use is a universal call,
  since a `FOR ALL` is solved against carried types when the call runs, and a
  functor quantifies itself,
  `EXPR FOR ALL #[Elt] #(MAKESET elem :(Ordered WITH {Carrier = Elt}))`, so
  [dispatch](../../src/dispatch/README.md#selection) solves `Elt` through the module's member types as it
  solves through a list's element type.
- *Type identity — decided.* A `NEWTYPE` is the digest of its name and schema
  ([recursive groups](../../src/type_lattice/README.md#recursive-groups-identity-is-the-scc-not-the-declaration)),
  so two declarations with one name and schema are one type wherever they are
  written, and a `NEWTYPE` in a functor body is one type across the calls that
  instantiate it alike. It is constructed and matched wherever its name
  resolves; restricting that would protect nothing, since any scope can declare
  the same type. Hiding is `:|`'s alone: a carrier has no constructor and no
  pattern, only the barrier wraps and unwraps it, and naming it lets code
  dispatch on it and nothing more.
- *What an opaque carrier is keyed on — decided.* The root implementation a view
  reaches, the signature instance solved relative to that root, and the
  parameter's name, so two parameters solved to one type still get two carriers.
  An ascription is checked against the surface of the module it is written on,
  so a view never admits what its source hides, and built from the root, so a
  view of a view wraps each value once. Opaque ascription is therefore
  idempotent: `(ints :| Counter) :| Counter` has `ints :| Counter`'s carrier and
  root members, and the two are interchangeable though two module values stay
  incomparable. `:!` shows the source's view, never the root's. A functor whose
  result is a `:|` view births a module per call, so each call's carrier is
  fresh; a type two calls share is a transparent view's `NEWTYPE`. A key per
  ascription site is unsound: one `:|` in a function called over two modules
  would give both one carrier.
- *A signature annotation on a module binder — decided.*
  `MODULE <name> :<Sig> = (<body>)`, and `GROUP <name> :<Sig> <mode> = (<body>)`,
  optionally name a signature the binder's module satisfies. The annotation
  checks as a `:Sig` slot does, structurally and without narrowing — unlike
  OCaml's `module M : S`, which seals — so ascription stays the only narrowing
  and the bound value is the module the body built. The check is ascription's
  relation run for its verdict when the statement runs, since the
  self-signature exists only once the body has run; it solves a head quantifier
  and discards the solution, and a `WITH` application pins as it does anywhere.
  The one thing the annotation hands the body is each keyworded member head's
  ranking, as a bucket declaration visible to the definitions written there
  ([keyworded uses](../../src/scope/README.md#keyworded-uses)), so a module meeting a signature whose members are
  ranked does not repeat the ranking. A module without an annotation is checked
  only where it is ascribed or passed to a `:Sig` slot.
- *Leaving a parameter unpinned — open.* Whether an application may pin some
  parameters and leave the rest quantified, `Pair WITH {First = Number}`
  leaving `Second` open, which relaxes the refusal of an application missing a
  key, for type constructors as for signatures.
- *Higher-kinded parameters — open.* A parameter ranging over type-constructor
  families, as a `Monad` signature over `(Type AS Wrap)` needs, lacks a spelling
  in the quantifier dict, and that spelling should be one with
  `UNION (Elem AS Option)`'s, one of the two respelled. Open with it: how an
  application headed by a variable pairs its arguments with a family's
  (positional in the variable's declared order is the simplest), and whether a
  builtin `List` or `Dict` stands as a family; `FN` never does, since its
  parameters are contravariant. A family satisfies a constructor parameter
  without matching parameter names.

## Dependencies

**Requires:**


**Unblocks:**

- [A compact type node table](compact-type-node-table.md) — `AbstractType` is
  gone before the node is sized.
- [Retire the old runtime](retire-the-old-runtime.md) — the module surface
  `machine` still owns.
