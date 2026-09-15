# Circular values

Data values that refer to themselves, born in a knot beside the functions that
capture them.

**Problem.** [`function`](../../src/function/README.md#the-tie) ties a deferred-only
component as a [knot](../../src/memory/README.md#the-knot) only when every
member is a callable binder. A component with a data member — `LET a = [f];
LET f = FN <reads a>`, or a ring of tagged values — is refused, although the
scopes' visibility rule admits it
([src/scope/README.md](../../src/scope/README.md#visibility)): a container
cell can hold nothing but a value word, and a value word has no arm that names
a fellow knot node, so a container cannot be a node. `values` has no equality
that terminates on a cycle, no renderer that does, and no copy that rebases a
member reference held inside a container's cell run.

**Acceptance criteria.**

- A deferred-only component with data members is born as one knot, and a
  container that is a member holds its sibling references so that a copy of
  the knot rebases them.
- A deferred mention below a nested constructor (`LET a = {inner: [b]}` with
  `b` in `a`'s component) is either written into the knot or rejected where
  the shape is built, and the choice is documented.
- Every cycle in a data knot passes through a node whose memoized type is
  declared — a callable or a tagged nominal value — and the tie refuses a
  component whose derived-type nodes form a cycle with an `Untieable` that
  names the cycle.
- Each container node of a knot memoizes an exact, finite type, and
  `satisfies` over a circular value answers by it.
- `==` over two circular values is a bisimulation over `(knot, index)` pairs,
  and `PRINT` of one terminates.
- A copied circular value is the same graph at the destination.

**Directions.**

- *How a container cell names a node — decided.* A sibling reference inside a
  knot is an edge, since the knot's address does not exist until its run is
  filled; a container holding one is itself a node, and `Value`'s one
  knot-member arm — the arm a function already rides in — is what a reader
  holds, with the node saying whether it is a function or a container. A
  container node's cells are one value-or-edge type shared with a function's
  closure bindings.
- *Nested deferred mentions — decided.* Each constructor literal on the path
  from a member's root to a sibling mention is written as an anonymous node of
  the same knot; a nested constructor with no sibling mention below it stays an
  ordinary value.
- *Eager parts of a data member's right-hand side — decided.* The tie takes an
  evaluator keyed by part site and refuses, writing nothing, on a part it has no
  value for; the caller evaluates the part and ties again.
- *How a cycle prints — decided.* A node referred to again inside its own
  rendering is labelled at its first occurrence, `@0 = [1, @0]`, and every later
  occurrence prints as `@0`; labels are numbered per `PRINT` in order of first
  appearance, and a node that is no cycle target prints inline each time.
- *The memoized type of a container node — decided.* Recursion in a value's
  type goes through a nominal declaration: a callable's or a tagged value's
  memo is declared and exists before the knot does, so the tie derives each
  container node's memo in reverse topological order over the derived-type
  nodes, reading a declared handle at every cut. An ascription is no cut, since
  `retyped` stamps a structural type and no structural type names itself. So
  `LET a = [1 a]` is refused, and `NEWTYPE Ring = :{next :Ring}` with
  `LET a = (Ring {next = a})` ties.
- *A nominal construction's payload — decided.* In a two-part expression whose
  head is a type token, the head is an eager mention and the payload a
  constructor slot, so a tagged cycle reaches the tie rather than being an
  eager-cycle shape error. The tie checks the construction itself: the head
  names a newtype and the payload's derived type satisfies its representation.
  A type-constructor application in value position is classified the same way
  and refused at the tie; union-variant construction stays eager.

## Dependencies

**Requires:** none — [functions](../../src/function/README.md) ship the tie this item extends.

**Unblocks:** none — a leaf of the rewrite.
