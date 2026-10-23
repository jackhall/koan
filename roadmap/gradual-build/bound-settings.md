# Setting the build budget

**Optional.** A user-facing setting matters once the language is releasable.

**Problem.** How much of the build is kept live is a parameter of the
program's setup — today the build root's bound and every specialization's
([keys and eviction](../../src/build/README.md#references-between-the-trees)),
under [the build arena](build-arena.md) its one budget — and the interpreter
in `src/main.rs` passes a default. A program whose hot loop keys more cells
than the budget holds misses on every call, and nothing a user writes or runs
can widen it.

**Acceptance criteria.**

- A user sets the budget when running a program, and the arena keeps that
  much live.
- A budget below one cell is refused before the program loads, with a message
  naming the setting.

**Directions.**

- *Where the setting lives* — open. A command-line flag of the interpreter,
  or a declaration in the program's text. Recommended: a flag, since a budget
  prices a run and says nothing about what the program means.

## Dependencies

**Requires:**

- [The build arena](build-arena.md) — the budget this sets.

**Unblocks:** none.
