# Modules

Module values, their signatures, and the forms that read a module's members.

**Problem.** The rewrite's [scopes](../../src/scope/README.md) build a `MODULE`
body as a module shape — capturing, eager — and its activation is one shape
with a function's, but no value stands for the module once its body has run:
`Value` has no module arm, nothing elaborates a module's self-signature from
the members its activation binds, and the shape builder reports `USING …
SCOPE` unsupported because resolving a surfaced name statically needs the
module's signature. The `:|` and `:!` ascription operators, and the
constructor-application type expressions (`Pair {Key = Number}`, `Number AS
Wrap`) that [`elaborate`](../../src/elaborate/README.md#refusals) refuses, have no home either. The
old runtime's [`Module`](../../src/machine/model/values/module.rs) holds a bare
`&Scope` and a frozen member table, both of which the rewrite replaces.

**Acceptance criteria.**

- The callable parameter [functions](../../src/function/README.md) close gains a module
  node beside the function node, and the value word stays at twenty-four
  bytes.
- A module value reports its memoized type handle: its self-signature,
  elaborated from the members its activation binds.
- `USING … SCOPE` over a module whose signature is known statically resolves
  the surfaced names in the shape; over one whose signature is not, it
  requires an ascription at the site and is refused without one.
- `:|` and `:!` build a view module carrying the signature's members, with
  opaque members minted per application under `:|`.
- Constructor-application type expressions elaborate through `elaborate`.
- The old runtime's tutorial programs that use modules run on the rewritten
  stack and print the same output.

**Directions.**

- *A module is a knot node — open.* Closing the callable parameter with an enum
  over a function member and a module reference costs eight bytes the value
  word does not have; making the module a node of the same knot payload keeps
  the member at sixteen bytes and lets a module take part in a knot. Deciding
  for the node relaxes the eager-module rule
  ([src/scope/README.md](../../src/scope/README.md#visibility)) only if a
  module body must become deferrable, which nothing here requires.
  Recommended: the node.
- *A module body's context — decided, provisionally.* Eager, per
  [src/scope/README.md](../../src/scope/README.md#visibility): a function
  outside a module mutually recursive with one inside is an eager-cycle error.
- *`USING … SCOPE` over a module — decided.* The surfaced names come from the
  module's signature, which must be known statically at the `USING` site; a
  module whose signature is not requires an ascription there.
- *Where the closing lives — decided.* The closed callable moves from
  `function` to this layer, which is the top of the callable stack.

## Dependencies

**Requires:**

- [Dispatch](dispatch.md) — a module program runs only under dispatch.

**Unblocks:**

- [Retire the old runtime](retire-the-old-runtime.md) — the last of the language
  surface `machine` still owns.
