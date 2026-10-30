# Named code as a body

A callable's body written as the name of a quote, so code named once is the
body of several callables.

**Problem.** A callable's body is a written quote and nothing else. The role
table reads a body part as a quote (`Role::Body(kind).reading()` in
[`role.rs`](../../src/parse/builtin_shapes/role.rs)), the static check refuses a
name there, and `enter_body`
([`build.rs`](../../src/scope/shape/build.rs)) builds the body's shape from the
syntax under the part it visits
([three tiers](../../src/scope/README.md#three-tiers)). The load traces a
`LET`-bound name to its written quote for an `EVAL` (`Walk::traced_code` in
[`walk.rs`](../../src/dispatch/walk.rs)), but the shape builder runs first and
consults nothing of the kind. So code that serves two callables is composed into
each declaration and `EVAL`ed,
`EVAL #((EXPR #(AREA w :Number h :Number) -> Number = #($..body)) …)`, which
confines the callable to the `EVAL`'s shape — no overload is visible past it
and no module exports it — and reports the body's refusals where the `EVAL`
runs rather than at load; a `$..` splice in a body written in place is refused
outright.

**Acceptance criteria.**

- A callable's body is a written quote or a lowercase name. The name is read as
  a value where the callable is born, that value's code is the body, and the
  callable registers and exports as one with a written body does.
- Where the load traces the name to a written quote, as it traces an `EVAL`'s
  operand, the body's shape is built at load and its refusals are reported
  there. Code the load cannot trace is built where the callable is born, found
  by its digest as `BUILD` finds a composition
  ([building code](../../src/scope/README.md#building-code)), its refusal kept
  and reported by the first call.
- Code named as the body of a callable with parameters `w` and `h` is written
  `#(\w * \h)`: the parameters fill its `\` marks, its own binders fill what
  they bind, and a hole in it is refused as unbound where the body is built
  ([holes and marks](../../src/scope/README.md#holes-and-marks)).
- The quote value and a body built from its code are two shapes over one text:
  the value keeps its carried type and its needs, and each callable the code
  names digests and compares as a callable with a written body does
  ([equality and knots](../../src/scope/README.md#equality-and-knots)).
- A name whose load-time type can never be code is refused at load; a value
  that is not code is an error where the callable is born.

**Directions.**

- *No name class and no sigil — decided.* Body position admits a quote and
  nothing evaluated, so a bare name there means named code and nothing else;
  inside a quote, `$` and `$..` already splice named code
  ([splicing](../../src/scope/README.md#splicing)). The load's typing is
  traced, so a sigil would mark nothing the load cannot tell, and `#body` sits
  one pair of parens from `#(body)`, the quote of a name.
- *What stands in body position — open.* A lone name, or any evaluated part,
  such as a member read `m.body` or a call answering code. Recommended: a lone
  name, which the trace follows and the quoted-or-bare rule states in a
  clause; a wider part composes with `$..` as it does today.
- *Keying the second shape — open.* A body built from a traced quote's syntax
  is keyed by its body site and numbered in walk order, as a nested shape is,
  or by its digest beside the compositions. Recommended: the body site, since
  the body is written, with its parameters, where the callable is.

## Dependencies

**Requires:** none — code composed at run time, which it shapes where it runs,
has shipped.

**Unblocks:** none — a leaf.
