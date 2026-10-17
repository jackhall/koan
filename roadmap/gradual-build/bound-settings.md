# Setting the specialization bounds

**Problem.** How many live specializations a build cell keeps is two
parameters of the load, the build root's bound and every specialization's
([keys and eviction](../../src/build/README.md#references-between-the-trees)),
and the interpreter in `src/main.rs` passes their defaults. A program whose
hot loop calls more keyed callees than the root bound misses on every call,
and nothing a user writes or runs can widen it.

**Acceptance criteria.**

- A user sets either bound when running a program, and the run holds that
  many live children per build cell.
- A bound below one is refused before the program loads, with a message
  naming the setting.

**Directions.**

- *Where the setting lives* — open. A command-line flag of the interpreter,
  or a declaration in the program's text. Recommended: a flag, since a bound
  prices a run and says nothing about what the program means.

## Dependencies

**Requires:** none — the bounds are parameters of the load.

**Unblocks:** none.
