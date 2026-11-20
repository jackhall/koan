# Module content and unstorable solves stated as questions

A read of a module binder's content, and a solve no stored solve stands for,
recorded as questions, so neither types its statement again under a binding.

**Problem.** Three marks stand for no question, and a statement one leaves
undecided **holds**: it is typed again under every binding
([resumption](../../src/build/README.md#resumption)). One, `ForAllBound`, is
[its own item](../types/run-bound-for-all-bounds.md). The other two:

- `ModuleContent`. A read of a module binder whose content the run builds — a
  view, a path or an unfold over a binder the load cannot see — marks the
  binder open on this kind (`note_content` in
  [`walk/reads.rs`](../../src/dispatch/walk/reads.rs)). The activation, memo,
  modules and paths suites assert those re-runs: about thirty of their tests
  fail where the kind stops holding. The resumption laws draw no binding of
  such a binder, so no law holds a resumption over one to a fresh load.
- `Unreproduced`. `judge::reproducible` and `judge::unreproduced`
  ([`judge.rs`](../../src/elaborate/judge.rs)) mark a solve a run may solve
  otherwise that no stored solve stands for: one over an argument the load
  left untyped, and one whose slots or variables reach 64, past the bits a
  stored solve (`Rule::Solved`) keeps. Neither the suite nor the example
  programs reach it.

Beside them, `header_picks` in
[`layout.rs`](../../src/knot/module/layout.rs) records no picks over a carried
type that is a derived name — a meet over names the lattice cannot spell —
where a fresh load with the binding declared records them. No test reaches it.

A runtime check stands where no type keeps a third mark from holding:
`left_unknown_unbounded` in [`unit.rs`](../../src/elaborate/unit.rs)
`debug_assert!`s that a type binder or a type expression the elaborator leaves
`Unknown` noted no run-bound name, and a release build marks such a statement
`ForAllBound`, so it holds. Several producers of `Elaboration::Unknown` can
still return it after reading a name — `closed_operands` and a meet over a path
variable in [`expression.rs`](../../src/elaborate/expression.rs), `closed` and
`handed`'s `None` in [`declaration.rs`](../../src/elaborate/declaration.rs) —
and the suite and the laws reach none of them over a name.

**Acceptance criteria.**

- No `OpenKind` but `ForAllBound` holds: `ModuleContent` and `Unreproduced`
  are each gone or a `Question`'s kind, as the test over every kind checks.
- A read of a module binder whose content the run builds records a question
  about the binder, and a second binding that keeps it types no statement
  again.
- A solve over an argument the load left untyped, and one over 64 or more
  slots or variables, marks no holding kind, and the program loads and prints
  as a fresh load with its bindings declared does.
- Header picks over a carried type that is a derived name equal a fresh
  load's with the binding declared.
- The resumption laws draw a binding of a module binder whose content the run
  builds, and resume equals restart over it.
- The activation, memo, modules and paths suites pass with no test asserting
  a re-run for module content, and no statement of the example programs holds.
- `left_unknown_unbounded` and its check are gone: a type binder or a type
  expression over a run-bound name elaborates to a type, a derived name or a
  refusal, and the elaborator's error for an expression it leaves unknown
  cannot arise once a run-bound name is read.

**Directions.**

- *A read of module content — open.* Recommended: module substitution for
  every content read, the binding written into the binder's tree as it is
  into a path's classes ([derived names](../../src/build/README.md#derived-names)).
  Else a question per read, naming the content it read.
- *A solve over 64 or more slots or variables — open.* A stored solve keeps
  its slots in a slice rather than a word, or the load refuses a shape that
  wide.
- *A solve over an untyped argument — open.* The argument's contribution is
  the type its value carries, written as a name the binding answers, or the
  solve is stored without it and asked again where the argument is bound.

- *Retiring the unbounded check — open.* Split the error the type-binder and
  type-expression paths return, so `Unknown` is unrepresentable after a
  run-bound read; or turn each producer that reads a name into a derived name
  or a question.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Build over the binding graph](build-over-the-binding-graph.md) — nothing
  is typed again once no unit holds.
