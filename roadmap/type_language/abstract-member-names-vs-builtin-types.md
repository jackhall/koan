# Abstract member names versus builtin type names

The `Type` convention the module docs teach is rejected by the unshadowable-builtins rule.

**Problem.** Every design doc that teaches the module surface names the principal abstract
type member `Type` — [modules.md](../../design/typing/modules.md) (`MODULE IntOrd = ((LET
Type = Number) …)`, `SIG OrderedSig = ((TYPE Type) …)`),
[tokens.md](../../design/typing/tokens.md) ("the convention is `LET Type = ...` for the
principal abstract type"), [functors.md](../../design/typing/functors.md), and
[implicits.md](../../design/typing/implicits.md). Neither form runs. `Type` is a builtin
type name, and builtins are immutable and unshadowable in either channel
([lookup-protocol.md](../../design/typing/lookup-protocol.md)): a MODULE-body `LET Type =
Number` and a SIG-body `TYPE Type` both raise `Rebind` ("name 'Type' is already bound in
this scope") through the
[`shadows_builtin_type`](../../src/machine/core/scope.rs) consult. The whole test corpus
sidesteps the collision by naming the member `Carrier` or `Elem`, so the contradiction has
never been caught by a test — it lives only between the docs and the implementation. The
member lookup itself would be unambiguous either way: `Bindings::lookup_member` is
deliberately module-own and never falls through to the builtin root, so `IntOrd.Type` could
only ever mean the module's own member.

**Acceptance criteria.**

- The forms the design docs teach as canonical (`MODULE M = ((LET Type = …) …)` and
  `SIG S = ((TYPE Type) …)`) run, or no design doc names `Type` as a member.
- A koan test pins whichever end state ships: either the declaration succeeds and
  `M.Type` reads back the member, or the diagnostic names the collision and the docs use a
  non-colliding member name.

**Directions.**

- *Which side gives — open.* (a) Relax the unshadowable-builtins rule for module and SIG
  child scopes: a member name lives in a module-own namespace that no lexical lookup falls
  through to, so a member named `Type` shadows nothing a reader could confuse it with. (b)
  Keep the rule and retire the `Type` convention from the docs in favour of the names the
  tests already use (`Carrier`, `Elem`). Recommended: (a) — the convention is load-bearing
  for readability (`Er.Type`, `Mo.Type` read as "the module's type"), and the rule it
  violates is protecting a lookup path that module-own resolution never takes.

## Dependencies

**Requires:** none — the collision is independent of the naming flip (a snake_case module
name does not change what its *members* may be called).

**Unblocks:** none tracked yet.
