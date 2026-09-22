# Modules

Module programs: the ascription and member-reading forms, running.

**Problem.** The [module layer](../../src/knot/module/README.md) holds the module
value, its self-signature, and the doors `:|`, `:!` and `USING … SCOPE` name,
each exercised over an activation whose members a test binds by hand. Nothing
evaluates them. An ascription is an expression, a member read is an attribute
form, and a name a `USING` surfaces resolves to a coordinate a running reader
has to redeem — each of which waits on [dispatch](dispatch.md) to choose the
callable a keyworded expression runs. No module program runs on the rewritten
stack, and the old runtime's [`Module`](../../src/machine/model/values/module.rs)
is the last of the language surface `machine` still owns.

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

## Dependencies

**Requires:**

- [Dispatch](dispatch.md) — a module program runs only under dispatch.

**Unblocks:**

- [Retire the old runtime](retire-the-old-runtime.md) — the last of the language
  surface `machine` still owns.
