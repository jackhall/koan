# An exhaustive check

An opt-in load that walks every binding the text passes and keeps only the
refusals.

**Problem.** [The trace](../../src/build/README.md#the-trace) leaves untraced every site a body
with more than 8 bindings passes a slot on to, and every site below them,
and the load walks only what the trace names
([where facts are decided](../../src/build/README.md#where-facts-are-decided)). A binding at such a site is first walked at run, so a refusal
in it faults the call there rather than refusing the load
([refusal](../../src/build/README.md#refusal)). Whether a program is refused
at load thus depends on how many bindings a body takes. The trace's
fold reaches every one of them from the text, and the cap bounds only what
the load keeps.

**Acceptance criteria.**

- Under an opt-in flag, the load enumerates every binding the trace's fold
  reaches, with no limit, walks each body under each one over a layer it
  then drops, and keeps only the refusals. A refusal there refuses the load,
  as a traced layer's does.
- The pass holds at most one walked layer at a time, beside the digests of
  the bindings it has visited.
- A test: a body over the cap whose ninth binding refuses loads without the
  flag, its call faulting at run, and is refused under the flag.
- Without the flag, the load is unchanged.

**Directions.**

- *Opt-in — decided.* The default load stays bounded, and CI runs the flag.
  Whether a program is refused at load then depends on the flag, as without
  it it depends on the cap.
- *Where the flag is named — open.* A flag of the interpreter, or a setting
  beside [the build budget](../features/bound-settings.md). Recommended: a
  flag, since it prices a load and says nothing about what the program
  means.
- *A binding an earlier layer admits — decided.* A binding that keeps every
  question of a witnessed layer the check laid has that layer's refusals
  ([resumption](../../src/build/README.md#resumption)), so it costs an
  admission test, not a walk.
- *A unit checked once per projection — deferred*, designed once a
  program's count shows a product. A unit's facts depend only on the names
  it waits on, so a unit reading only `a` refuses alike under every binding
  with that `a`. Checking each unit once per distinct content at its names
  leaves the product only for units reading several varying names, such as
  `MODULE built OVER #[a b]`. It needs the one walk split per unit.

## Dependencies

**Requires:** none.

**Unblocks:** none.
