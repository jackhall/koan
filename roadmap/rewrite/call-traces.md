# Call traces

**Problem.** An [error value](../../src/program/README.md#errors-and-output) is a
tagged value of the builtin `Error` over `{message :Str}` and nothing else, and
every evaluation and frame that receives one passes it on unchanged
([dispatch](../../src/dispatch/README.md#errors)). So an uncaught error prints
only `error: <message>`: nothing says which calls it passed through, and an
error raised deep in a recursion reads the same as one raised at the top level.
The old runtime printed an `in …` line per frame beneath the message, and
[tutorial 09](../../tutorial/09-errors.md)'s `TRY` section, still pending on the
rewrite, describes a `frames` field every caught error carries.

**Acceptance criteria.**

- An uncaught error prints, beneath its `error: <message>` line, one line per
  frame it passed through, innermost first, each naming the callee and the
  `path:line:col` of the call; an error raised at the top level, outside any
  call, prints none.
- A caught error's record exposes the same frames, as a field an arm reads.
- Tutorial 09's snippets that show a trace or read `frames` run on the
  rewritten stack and print the output the tutorial shows.

**Directions.**

- *Where a frame is recorded — open.* A frame finishing with an error value
  could append itself as it passes the error on, which gives every error a
  fresh value per frame; or the error could carry a link to the frame chain
  that raised it, read only when it is printed or caught.
- *A frame a tail hop replaced — open.* A tail call runs in its caller's place
  ([tails](../../src/program/README.md#frames-contracts-and-tails)), so a trace
  built from live frames omits every hopped one. Either a trace shows only the
  frames still live, as a tail-calling language's does, or a hop records the
  call it replaced at some cost per hop.
- *What a frame line names — open.* The old runtime named the callee by its
  type and the call as written; a keyworded call might instead name its bucket
  key.

## Dependencies

**Requires:**

- [Control expression shapes and errors](control-and-errors.md) — a caught error's record is `TRY`'s and `CATCH`'s.

**Unblocks:** none — a leaf.
