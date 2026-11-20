# A run-bound name bound to a signature type

A binder declared at a run-bound name that a binding makes a signature type,
read as a module binder under that binding as a fresh load reads it.

**Problem.** Whether a binder declared at a type is a module binder is decided
at load: `judge::module_node` ([`judge.rs`](../../src/elaborate/judge.rs))
rules that a binder whose type is a variable bounded by no signature is none,
whatever a binding gives the variable. A fresh load with the binding declared
decides it from the bound type. So under

```koan
EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(
  LET s = (x :! Elt)
  s
)
```

binding `Elt := Module` leaves `x` a value binder in the resumption, while a
fresh load with `LET Elt = :Module` declared makes `x :Elt` a module binder;
nine restart laws fail on such a binding. The law generator draws no binding
of a run-bound name to a signature type, so the laws hold over bindings that
keep each binder's kind.

**Acceptance criteria.**

- `SUBJECT` above, resumed under `Elt := Module`, gives the facts a fresh load
  with that binding declared gives.
- The law generator draws bindings of a run-bound name to a signature type,
  and resume equals restart over them.

**Directions.**

- *How a binding changes a binder's kind — open.* (1) A binder declared at a
  variable that a signature may bind asks whether it is a module binder, a
  question a binding answers, and a binding that changes the answer re-runs
  the binder's readers. (2) A binding of an unbounded name to a signature
  type is refused, so no binding changes a binder's kind.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
