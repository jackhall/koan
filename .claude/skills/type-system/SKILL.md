---
name: type-system
description: Use this skill before any work that touches koan's type system — editing or reviewing `lattice/src/types` (koan's `type_lattice`), the elaborator's type channel, dispatch's static types or selection, a value's carried type or retyping, or answering a design question about types, generics, `FOR ALL`, signatures or gradual typing. Holds the lattice's invariants, the checklist to run before a change, and the tells that a change breaks a law.
---

# type-system

The type lattice is a mathematical object. Its laws are not style: every union,
overload set, cache key and load-time verdict assumes them, and a broken law
fails somewhere else, later, as a bug that looks unrelated. Treat a law as you
would memory safety.

## Read first

- [`lattice/src/types/laws.md`](../../../lattice/src/types/laws.md) — each law,
  what rests on it, and a worked example of what breaks without it. Read the
  section for every law your change touches, not the headings.
- [`design/gradual-typing.md`](../../../design/gradual-typing.md) — how a type
  goes from a declaration, through the load, to a call.
- [`design/quantified-types.md`](../../../design/quantified-types.md) — where a
  `FOR ALL` may live, how a signature keeps a scheme safe, and the workarounds
  and limits for higher-ranked types. Read it before any work on signatures,
  modules or quantified callables; do not re-derive it.
- The part of [`lattice/src/types/README.md`](../../../lattice/src/types/README.md)
  that owns what you are changing.

## The invariants

1. **A value's type is concrete.** An unsolved variable is a pair of ends,
   `[lower, bound]`, or a solved type. Never read a variable as its bound.
   `KType` is concrete, `Parametric` may hold a variable, `Scheme` is a
   quantified callable's type.
2. **The order is a partial order by handle, over concrete types only.** It
   holds no unification. Solving belongs to *fits* and the unifier, at load and
   at the call.
3. **`join` and `meet` are exact**, and the four lattice laws hold by handle.
4. ***Fits* is a preorder containing the order. Nothing is built from it.**
5. **A solve collects, then solves.** No answer depends on slot order.
6. **A load-time verdict holds at every run.** Static types are intervals;
   every rule maps both ends.
7. **One type, one handle.** Canonical shapes are part of identity.
8. **The laws are the tests.**

## Before a change

Answer each in your plan, in a sentence:

1. Which laws does this touch, and why does each still hold?
2. Does anything parametric reach the order, `join`, `meet` or a union? It must
   not.
3. Does anything read a variable as its bound, or read one end of an interval?
4. Is anything built — a union, a set, a key — from a *fits* answer?
5. Can the change live downstream of the lattice? A change to the lattice
   (`lattice/src/types`) itself needs the user's approval first: stop and ask.
6. Which generators and laws in `tests/properties.rs` must grow to cover it?

A new language rule or type spelling is the user's decision. Enumerate the
options and ask; do not pick one to make a test pass.

## Tells that a change is wrong

- a raw `Handle` wrapped into a typed handle outside the lattice's own doors;
- a new `TypeRegistry::concrete` call with no named invariant behind it;
- a concrete type compared with a parametric one by reading the variable as
  its upper bound: this is the usual way transitivity is lost;
- a scheme or variable inside a type a value carries or a container is built
  from: parametric types are contagious, and weaken every type built over them;
- `erase_rigid` where `bound_above` or an interval is meant;
- a structural descent beside the two walk drivers, or a second guard set;
- a law restated "up to equivalence", or a generator narrowed until it passes;
- a hand-written test where a law would state the property;
- an answer that changes when two members, slots or declarations swap;
- a runtime check standing in for something the typed handles could refuse.

If a law fails, the lattice is wrong or the law is. Say which and why before
changing either, and take a law change to the user.

## Verification

- `cargo test -p lattice` for a lattice change; the property suite is the
  gate, and one run decides.
- The `verify-koan` skill before handing off.
- `PROPTEST_CASES=16384 tools/verify.sh --total` for a change to a relation or
  the unifier, where the user asks for the long sweep.
