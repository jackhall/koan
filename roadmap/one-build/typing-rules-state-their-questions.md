# Every typing rule states its question

A typing rule's judgement about a bound name stored as a question with
finitely many answers, so no statement is typed again under a binding.

**Problem.** A typing rule reaches a name the run binds through a judgement
function ([`judge.rs`](../../src/elaborate/judge.rs)): 33 of them, called
from about 230 places in the rules. Those that answer with a verdict, a
bool, a kind or a stamp record a
[`Question`](../../src/scope/question.rs) beside the answer taken, in nine
kinds, and a later binding that keeps every question of a statement reuses
its typing. The rest hand the rule a type, and the rule branches on it in
code. Such a statement **holds**: it is typed again under every binding
([resumption](../../src/build/README.md#resumption)). Ten of the eighteen
[`OpenKind`](../../src/scope/facts.rs)s have no `Question`: `Settles`,
`Solve`, `Instance`, `EndRead`, `TypeNodeOfVariable`, `TypeExpression`,
`Unclosed`, `Overlap`, `TypeValue` and `ModuleContent`. A read that finds a
module binder, a path or a class by content holds its statement, hit or miss,
and so does a `never_satisfies` that decides *never*, since its refusal's
wording reads an end. Nothing counts how many of a real program's statements
hold.

**Acceptance criteria.**

- Every judgement a typing rule makes about a name the run binds is a
  `Question` with finitely many answers, or yields a type written over the
  name, which a binding is written into. No judgement function hands a rule
  a type read off a variable's end, and no `OpenKind` lacks a `Question`.
- No statement holds. A test over the example programs counts no statement
  typed again under a binding that keeps its questions.
- A read by content records a question about the binder it finds, and holds
  nothing.
- A refusal's wording is rendered by writing the binding into the types its
  question stored, so a `never_satisfies` that decides *never* is a question
  like any other.
- A count, over the example programs, of the questions each kind records per
  statement, taken before the change and after.
- The property laws hold
  ([properties](../../src/build/README.md#properties)), and every program
  prints as before.

**Directions.**

- *A rule that cannot state its question — decided.* It is restated before
  this item ships. No rule keeps an escape that types its statement again,
  and none is left to the run.
- *A type the lattice cannot yet spell — decided.* A piece of what a
  variable is bound to, or any other type a rule needs written over a name,
  is a lattice change with an item of its own in
  [the types project](../types/README.md), as
  [projection types](../types/projection-types.md) is.

## Dependencies

Compressing the questions one name collects is
[unplanned work](README.md#unplanned-work).

**Requires:**

- [Projection types](../types/projection-types.md) — a read out of a
  variable's value named as a type, where a rule is handed one today.

**Unblocks:**

- [The build over the binding graph](build-over-the-binding-graph.md) — the
  questions its load stores are these.
