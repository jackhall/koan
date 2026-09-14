# Scopes

Koan's lexical environments: the layer that answers what a name means at the
point it is read, over the values in [`values`](../values/README.md) and the
types in [`type_lattice`](../type_lattice/README.md).

A scope resolves value names and type names. Keyword lookup — choosing a
callable for a keyworded expression — is dispatch, the layer above, and it
extends the resolution described here rather than replacing it.

## Three tiers

A scope is built in three tiers, each at the moment its contents become known.

- **The shape** — one per body, built once in program storage and shared by
  every scope instance of that body. It records the value names and type names
  the body declares, each with the lexical position its binder writes at, and
  it resolves every name the body reads.
- **Closure bindings** — one run per closure, built when the closure is born.
  Each name the body reads from an enclosing scope is copied in shallowly: the
  binding's value word, not a deep copy of what it points at. A closure's
  birth waits on any captured name whose binding is still a placeholder, so a
  placeholder is never copied.
- **Per-call bindings** — one activation per call, laid down in the call's
  frame region: the closure bindings copied verbatim, followed by one
  placeholder slot for each parameter and each local the shape declares.

Values are immutable, so a shallow copy of a binding means the same thing as a
reference to it: every copy names the same value, and what the copy retains is
the region that value lives in.

## Resolution

Every name a body reads resolves when its shape is built, to one of three
coordinates:

- a slot of the body's own per-call bindings,
- a slot of its closure bindings, or
- an index into the builtin table.

Reading a name is then one indexed load. No runtime walk visits enclosing
scopes, and no binding holds a reference into another scope: a coordinate is
computed from the name at the reading site, and what a slot holds is a value.

## Visibility

A binding is visible to a reader when its declared position is strictly less
than the reader's, compared within the scope that declares it. A parameter
writes at position `0` and a body's statement `i` at `i + 1`, so a body sees
its parameters and its earlier statements, and a binder never sees its own
right-hand side.

For a name declared in an enclosing scope, the reader's position is the
position of the definition that encloses it — fixed where the body is written,
not where it is called. A body therefore never sees a later sibling of its own
definition, and the capture set a shape computes is exactly the set of
bindings visible at the definition site. This rule is provisional: mutual
recursion between functions needs a definition window, and the rule is held
open until functions exercise it (see [Open work](#open-work)).

Types declared together in one module body share one position, so a group of
mutually recursive types is visible to each of its members in any source
order.

## Two channels

A value name and a type name are different key types, so the value channel and
the type channel cannot collide by construction. A name whose text classifies
as neither is rejected where the text is classified, before any scope sees it.

**Builtins are immutable and unshadowable.** A user binding whose name collides
with a builtin's, in either channel, is a rebind error at any depth, never a
shadow. Because no scope can hide a builtin, a builtin name resolves in the
shape to an index into the builtin table, and a read goes through a base
pointer to that table carried in the activation's header. An activation copies
no part of the builtin table.

## Placeholders and writes

A slot is written once. Its binder replaces the placeholder in place, and
nothing rewrites it after that, so a binding is as immutable as the value it
holds. A read of a visible slot that is still a placeholder does not fail: the
reader waits for the binder to commit. What the waiting reader is parked on is
the scheduler's concern; the scope reports the slot as pending.

## Names that arrive at run time

Two forms introduce names no shape can see.

- **`EVAL`** resolves the names of the code it evaluates against the scope it
  appears in, by name: a search of each shape's declared names, from the
  innermost scope outward, with the same visibility rule. A by-name resolution
  picks the same binding the shape's coordinate would. Resolving outward needs
  the enclosing scopes to still exist, so the closure of a body containing
  `EVAL` retains its defining scope.
- **`USING … SCOPE`** takes the names it surfaces from the module's signature,
  so a shape resolves them like any other name. The module's signature must be
  known statically where `USING` appears; a module whose signature is not
  requires an ascription there.

## Memory

An activation is bump-backed in its frame's region and `Drop`-free, so a
frame's death releases it with the region. It holds no pointer into itself —
its shape lives in program storage, its builtin table outlives every frame,
and its slots hold values — and the closure bindings it copies hold no
placeholder, so an activation is copied by copying its bytes.

## The import rule

`scopes` names `crate::values`, `crate::type_lattice`, `crate::memory` and
`crate::parse`, and no scheduler type. The scheduler reaches scopes through
its embedder; scopes never reach the scheduler.

## Open work

- [Scope on values and types](../../roadmap/rewrite/scope-on-values-and-types.md)
  — the module this document describes.
- [Callable values](../../roadmap/rewrite/callable-values.md) — closure
  bindings born from a function value, and the definition windows that settle
  the provisional visibility rule.
- [Dispatch](../../roadmap/rewrite/dispatch.md) — keyword lookup over scopes.
