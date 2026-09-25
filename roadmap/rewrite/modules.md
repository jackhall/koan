# Modules

Module programs: the ascription and member-reading forms, running.

**Problem.** The [module layer](../../src/knot/module/README.md) holds the module
value, its self-signature, and the doors `:|`, `:!` and `USING … SCOPE` name,
each exercised over an activation whose members a test binds by hand. Nothing
evaluates them. An ascription is an expression, a member read is an attribute
expression shape, and a name a `USING` surfaces resolves to a coordinate a running reader
has to redeem — each of which waits on [dispatch](dispatch.md) to choose the
callable a keyworded expression runs. No module program runs on the rewritten
stack, and the old runtime's [`Module`](../../src/machine/model/values/module.rs)
is the module surface `machine` still owns.

**Acceptance criteria.**

- An ascription expression evaluates: `:|` and `:!` run the view door where they
  are written and bind the view module it hands back.
- A member read evaluates: `m.f`, and a name surfaced by `USING … SCOPE`, read
  the [member run](../../src/knot/module/README.md#layout-order) the module holds
  where the reader runs, and a member that is itself a knot member crosses to
  the reader priced as its knot.
- A coerced function member runs: calling the
  [barrier](../../src/knot/module/README.md#members-are-born-coerced) an opaque view
  holds rewrites each argument from the view's
  types to the source's, runs the underlying function, and rewrites the result
  to the view's types where the call returns.
- The old runtime's tutorial programs that use modules run on the rewritten
  stack and print the same output.

**Directions.**

- *Where the doors live — decided.* In the
  [module layer](../../src/knot/module/README.md). This item evaluates them and adds
  no door of its own.
- *Parameterized signatures — open.* A signature's type member — `TYPE Carrier`,
  `TYPE (Carrier UNDER Number)` or `TYPE (Type AS Wrap)` — is a type the module
  chooses once and the ascription hides or shows: `:|` mints it fresh, which is
  what the [barrier](../../src/knot/module/README.md#members-are-born-coerced),
  the abstract member's nonce and the signature meet over abstract members
  serve, and where the [unplanned work](README.md#unplanned-work)'s barrier and
  meet holes sit. The alternative is a signature quantified at its head,
  `SIG Ordered FOR ALL #{Carrier: Bound} = #[…]`, so that
  `Ordered WITH {Carrier = Number}` is an application, a module satisfies an
  instance by its member types alone, representation hiding is a `NEWTYPE`
  inside the module, and `:|` and `:!` differ only in which members they show;
  no abstract member exists, and the barrier and the nonce go with it. A
  functor then quantifies itself,
  `EXPR FOR ALL #[Elt] #(MAKESET elem :(Ordered WITH {Carrier = Elt}))`, and
  [dispatch](dispatch.md) solves `Elt` through the module's member types as it
  solves through a list's element type; a higher-kinded parameter needs a
  spelling the quantifier dict lacks; and the parameter spelling should be one
  with `UNION (Elem AS Option)`'s, one of the two respelled. Recommended: the
  parameterized signature.

## Dependencies

**Requires:**

- [Dispatch](dispatch.md) — a module program runs only under dispatch.

**Unblocks:**

- [Retire the old runtime](retire-the-old-runtime.md) — the module surface
  `machine` still owns.
