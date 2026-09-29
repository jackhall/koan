# Faults and call traces

**Problem.** A run-time refusal is an
[error value](../../src/program/README.md#faults-and-output) built by
[`Program::error`](../../src/program/record.rs): a tagged value of the builtin
`Error` over `{message :Str}`, its message rendered into text in the region it
is raised in. Every unit that receives one tests its tag and stops, and a frame
finishes with it, so it climbs the frame chain as a value while each frame it
leaves is released. So an uncaught error prints only `error: <message>`:
nothing says which calls it passed through, and an error raised deep in a
recursion reads the same as one raised at the top level. Every refusal renders
its text whether or not anything prints it, and a field holding a value could
not outlive the frame that raised it. The old runtime printed an `in …` line
per frame beneath the message.

**Acceptance criteria.**

- An evaluation, a frame and a block each finish with a value or a **fault**. A
  fault is no koan value, and no unit tests a value's tag to find one.
- A fault is one of a closed set of kinds, the refusals a running program can
  meet, each carrying structured fields — types, names, code, or where a value
  lies in a kept frame — and no rendered text.
- Every frame a fault passes through stays unreleased until the fault is caught
  or the host drops the uncaught outcome holding it, and the graph is empty
  after either.
- An uncaught fault prints `error: <message>`, rendered from its fields through
  the program, and beneath it one line per live frame it passed through,
  innermost first, each naming the callee and the `path:line:col` of the call;
  a fault raised at the top level, outside any call, prints none.
- A trace counts the calls tail hops replaced, per callee, and shows a callee
  hopped to more than once as one line with its count.

**Directions.**

- *A fault is no value in flight — decided.* User code cannot raise an error;
  a function that can fail returns a `Result`. Nothing in the language reads a
  fault between its raise and its catcher, so it travels as the runtime's own
  and becomes a koan value only where it is caught or printed.
- *The order kept frames are released in — decided.* A
  [`release`](../../cellgraph/README.md#verbs) whose cell still has an
  undisposed tree cell under it outlives the call until that cell disposes, so
  the runtime releases kept frames in any order.
- *How the runtime finds the frames it kept — open.* `cellgraph` cannot list
  them: tenants stand in no relation, and several tenant frames share one host.
  Recommended: each frame that passes a fault on writes a link record into its
  own kept region — its callee, its call site, its hop counts and the handle of
  the frame inside it — and the fault carries the outermost link's handle; the
  catcher walks the links, builds the trace, and releases each frame.
- *A debug mode — open.* Recommended: none. Keeping frames is part of the
  semantics and holds only frames live at the raise, until the catch; an
  uncaught outcome's frames are the host's to drop. A mode would only keep what
  a normal run discards, and turning tail hops off would change whether a
  program runs in constant space.
- *Hop counts — open.* The counts ride the contract a tail hands over
  ([tails under a contract](../../src/dispatch/README.md#tails-under-a-contract)).
  Undecided: whether a count is keyed by callee or by callee and call site;
  the order counted callees render in, since mutual recursion's interleaving is
  lost; whether a live frame's own callee folds into its count (`f ×1000`
  rather than `f` over `f ×999`); and whether runs of identical live lines from
  a recursion outside tail position collapse when rendered.
- *What a frame line names — open.* The old runtime named the callee by its
  type and the call as written; a keyworded call might instead name its bucket
  key.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Catching errors](catching.md) — what `TRY` and `CATCH` catch, and the frames they read.
