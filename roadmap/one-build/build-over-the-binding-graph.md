# The build over the binding graph

Koan's load laying [the binding graph](../../design/binding-graph.md), and
the build and every run cell executing it.

**Problem.** A decision that waits on a binding is made by typing a body
again: a resumption is rooted at one body and visits its statements in order
([resumption](../../src/build/README.md#resumption)). What it decides is
stored per body and per binding, in five homes with five lifetimes and keys:
the load's base layer, a layer the load lays for a traced binding, a
settlement in the depot, a composition, and a frame's activation with its
decision memo
([where facts are decided](../../src/build/README.md#where-facts-are-decided)).
Neither grain is where content arrives.

- **Through a call.** The trace follows a name only where a call passes it on
  bare, and names no binding of a body with more than eight
  ([the trace](../../src/build/README.md#the-trace)). Under

  ```text
  EXPR FOR ALL #[Elem] #(KEEP x :Elem) -> Elem = #(x)
  EXPR FOR ALL #[Item] #(WRAP y :Item) -> :(LIST OF Item) = #(KEEP [y])
  PRINT (WRAP 5)
  ```

  `KEEP`'s facts under `Elem := LIST OF Number` wait for its frame, though
  the text fixes them.
- **Inside a body.** Under

  ```text
  EXPR #(SHOW n :Number) -> Str = #("number")
  EXPR #(SHOW s :Str) -> Str = #("string")
  EXPR FOR ALL #[Elem] #(REPORT x :Elem code :Any) -> Str = #(
    LET first = (SHOW x)
    LET built = (EVAL code -> Module)
    SHOW (built.sample)
  )
  PRINT (REPORT 5 #(MODULE made = (LET sample = "hi")))
  ```

  the frame that reaches `LET built` walks `REPORT`'s body again in its
  activation, typing each statement that waits on `built`, and keeps what it
  decides in its own region, shared only along its tail chain's memo
  ([activations](../../src/build/README.md#activations)).
- **At a refusal.** A binding's refusal is raised at the resumption point for
  every body beneath it, whichever bind site its inputs arrive at
  ([refusal](../../src/build/README.md#refusal)).

**Acceptance criteria.**

- The load lays one binding graph per base through
  [the crate](../../bindgraph/README.md): the program's at the load, and a
  composition's when it is composed. What the text alone decides is laid with
  the graph and is no `SiteQuestion`.
- A bind site is a body's entry, or a place inside a body where a name's type
  arrives as the program runs. In the second program the
  graph has a `BindingNode` at `REPORT`'s entry owning `SHOW x`, one at
  `LET built` owning `SHOW (built.sample)`, and none at `LET first`.
- Every `SiteQuestion` is stored with the types it judges, written over
  names, the `NameQuestion`s it asks and its arms, and an instance is made by
  asking them of a binding. A test counts the statements typed after the
  load, at build and across a run, and counts none.
- Every `NameQuestion` has finitely many answers, and no `SiteAnswer` holds a
  type with a binding written in. A test loops a callee through a new type
  each hop and counts no instance made once its answers repeat.
- The load makes instances from the root alone, along the edges of the calls
  the text selects and the arms its answers take, and an undecided answer
  follows no arm. `KEEP`'s instance under `Elem := LIST OF Number` in the
  first program is made at the load.
- A caller reads its callee's result through the name its statement binds:
  the declared return type, or the returned module's description, with the
  call's mapping written in, and otherwise an `AnswersName` edge from the
  callee's result. A test calls, with a type the text fixes, a callee whose
  last statement selects between two functors by its `FOR ALL` name, and the
  caller's read of a member of the result is answered at the load.
- A run cell reaching a bind site finds its instance by the view it is filed
  under, or makes it by the evaluation the load runs. A frame holds one
  handle per `BindingNode` owning a question in its body, at the width the
  load fixed for that `BindingNode`, and reaches its enclosers' through its
  closure. In the second program, two calls of `REPORT` whose modules'
  `sample` are both `Str` hold equal handles at `LET built`.
- A run under the crate's `perf` feature reports, over the example programs,
  the `NameQuestion`s asked and the stored questions evaluated.
- A name no answer types is a rigid: koan's domain gives it a lexical variable
  bounded by its declared type. In a recursion whose result waits on itself,
  a question reading that result is answered at the load for every type
  within the bound.
- Instances rest in the crate and nowhere else. No settlement, key, request,
  stamp, traced layer, witnessed layer, activation or decision memo remains,
  the depot holds compositions alone, and nothing of the build rests in a run
  region but a frame's handles.
- A call by name of a body with a `FOR ALL` name solves its group once, in
  its frame: a test counts `admitted` calls over one and counts one.
- No cap limits the instances the load makes, and a program whose bindings
  the load cannot bound is refused
  ([refusal](../../design/binding-graph.md#refusal)).
- A name declared `Any` asks no `NameQuestion`, and a `SiteQuestion` waiting
  on one is answered by the run cell that reaches it.
- A refusal is raised at the bind site that owns its question; a test binds
  a body's entry and a later bind site and observes each refusal at its own.
- An instance's answers equal those of a fresh load with its binding
  declared, for an instance the load makes and one a run cell makes, over
  generated programs and bindings; the monotone and confluent laws hold
  ([properties](../../src/build/README.md#properties)).

**Directions.**

- *The grain of work — decided.* A stored question evaluated, never a body or
  a statement typed again. Every judgement about a bound name is a question
  ([resumption](../../src/build/README.md#resumption)), save the three marks
  that hold, which
  [module content and unstorable solves stated as questions](module-content-questions.md)
  and [a `FOR ALL` bound over a run-bound name](../types/run-bound-for-all-bounds.md)
  take.
- *Where the load starts — decided.* At the root alone: the build does no
  work a run may never use.
  [An exhaustive check](exhaustive-check.md) starts from every entry under an
  opt-in flag.
- *Where a `BoundInstance` rests — decided.* In the crate: frozen with the
  load where the load made it, and in its agent's budgeted bump where a run
  cell did, dropped and rebuilt behind the crate's door
  ([the crate's instances](../../bindgraph/README.md#instances)).
- *A build tier in a run region — decided.* None. An instance names no type
  of a binding, and a type with a frame's content written in rests in
  [the lattice's tiers](../../design/type-tiers.md).
- *Compositions — decided.* A composition is a base with a graph of its own,
  laid when it is composed, resting in it and dropped with it.
- *A callee's result — decided.* An `AnswersName` edge carrying a type, taken
  from the callee's result name. No question reads another's answer, and no
  value is bound at build.
- *Settlements — decided.* This item removes them with the other homes; no
  item retires them first.

## Dependencies

**Requires:**

- [`bindgraph`](../../bindgraph/README.md) — shipped: the graph the build
  stores.
- [Module content and unstorable solves stated as questions](module-content-questions.md)
  — no unit re-runs over a module binder's content or an unstored solve.
- [A `FOR ALL` bound over a run-bound name](../types/run-bound-for-all-bounds.md)
  — no unit re-runs over such a bound.

**Unblocks:**

- [The run reads site answers](run-reads-site-answers.md) — the answers a
  run performs are the graph's.
- [A small union split where it is bound](small-unions-split.md) — a split
  is an `AnswerBranch` at a bind site.
- [A recursion that grows its type is refused](../types/growing-recursion-refused.md)
  — the refusal reads a `PassesName` cycle.
- [The build image](build-image.md) — the image writes the graph.
- [What the agent tier keeps](agent-tier-keeps.md) — nothing settles into
  the tier once settlements are gone.
- [An exhaustive check](exhaustive-check.md) — the pass starts where the
  load does not.
- [Captures and views traced](captures-and-views-traced.md) — restated over
  what a `PassesName` mapping may write.
- [Callees built by a call traced](built-module-members.md) — restated as an
  edge to a callee that is a call's result.
- [Warm composed types](warm-composed-types.md) — restated over a
  composition's own graph.
- [A head reading its parameters' types](../types/head-parameter-reads.md)
  — a `TYPE OF` is answered where its operand is bound.
