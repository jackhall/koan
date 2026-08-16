# Parse at runtime

**Problem.** [`EVAL`](../../src/builtins/eval.rs) evaluates a `KExpression` — a
`#(…)` quote the program text already contains — so runtime code assembly can
only recombine fragments written by hand at parse time. A string a program
builds while running has no route to become an expression, which is the gap
between "combine quotes with ordinary functions" and a metaprogramming story
that can assemble a declaration from data.

The missing capability is storage, not syntax. `crate::parse::parse` takes a
`ProgramBrand` ([`arena/frame.rs`](../../src/machine/core/arena/frame.rs)),
mintable only from a `ProgramStorage`, and the only one that exists is a local
in
[`interpret_with_writer_path`](../../src/machine/execute/interpret.rs).
Nothing reachable from a builtin's `BodyCtx` can obtain one, so no builtin can
parse.

The brand is the right gate rather than an obstacle to route around. An
expression's parts must live somewhere that outlives every holder and joins no
pin bundle — that is what lets
[`KObject`](../../src/machine/model/values/kobject.rs) call an expression cell's
reach `Owned` and answer `retains_home` false, and what lets the expression door
seal its cell with no member, with koan composing no reach description for an
expression at all. A builtin that reached for its step's own brand instead would
produce a node borrowing a region its holder outlives; `KExpression` is
covariant, so nothing in the type system would object. Threading the storage is
what makes the correct brand the reachable one.

**Acceptance criteria.**

- The runtime owns program storage for the length of the run: the
  `ProgramStorage` that `interpret_with_writer_path` stands up is held by the
  runtime (or its run frame) rather than by that function's stack frame, and is
  released when the run ends. Parse output from before the runtime existed
  stays valid — the storage is created first and released last, as now.
- `BodyCtx` and `FinishCtx` expose a `program_brand()` door beside `brand()`,
  returning `ProgramBrand<'a>`, so a builtin body reaches `parse` with no
  storage parameter of its own and no route to the wrong brand.
- A builtin that parses a string assembled at runtime yields a `KExpression`
  usable exactly as a parsed-from-source one: it dispatches, binds to a
  `:KExpression` slot, and survives the death of the frame that produced it —
  pinned by a test that drops the producing call and then evaluates the result.
- A runtime-parsed expression entering a container as a `KObject::KExpression`
  keeps the three reach answers above true, verified by the seam-equivalence
  battery (`tools/seam_equivalence.sh`) rather than by a new special case.
- Source registered by a runtime parse renders error frames the same as
  program text does: a parse failure inside a builtin surfaces a structured
  `KError` with a real span, not a panic.
- No `unsafe` is added, and no answer relaxes onto a runtime audit.
- Retention is documented where the door is: each runtime parse retains its AST
  and (through `source::register`) its source text for the rest of the run, so
  a program that parses in a hot loop grows program storage without bound. The
  door's doc states this; whether to cap it is out of scope here.

**Directions.**

- *Where the storage lives — open.* (a) A `ProgramStorage` field on
  `KoanRuntime`; (b) on the run frame, beside the run-root storage it already
  holds. (a) keeps it out of the frame lifecycle, which is the property that
  makes program storage exempt from being anyone's `home`; (b) puts it where
  scope-side code already looks. Recommended: (a).
- *Surface — open.* (a) A `PARSE` builtin taking a `Str` and returning a
  `KExpression`, composing with `EVAL` unchanged (`EVAL (PARSE s)`); (b) an
  `EVAL` overload admitting a `Str` operand directly. Recommended: (a) — it
  leaves `EVAL`'s semantics and its `TypeMismatch` contract alone, keeps one
  job per builtin, and makes the parse failure observable as its own value
  rather than folded into evaluation.
- *Scope of the parsed AST.* Free names in a runtime-parsed expression resolve
  at the site that evaluates it, exactly as a quote's do — this item adds no
  scoping rule of its own.

## Dependencies

**Requires:** none — `ProgramBrand` and `ProgramStorage` already exist in
[`arena/frame.rs`](../../src/machine/core/arena/frame.rs); this item threads
them to a reachable place.

**Unblocks:** none. [EVAL splices in place](eval-splices-in-place.md) is
orthogonal — it governs where a spliced declaration registers, for any
`KExpression`, however that expression was built.
