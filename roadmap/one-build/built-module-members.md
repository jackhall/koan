# Callees built by a call traced

A call whose callee is a call's result — a returned `FN`, a member of a
module a functor built — traced, and its layer laid by the load.

**Problem.** The walk traces a call site only where it names the callee: a
keyworded use resolves to a registration on the site's lexical chain, and a
call by name traces where its head's tree is a member of a knot of a callable
body ([the trace](../../src/build/README.md#the-trace)). A callee that is a
call's result has no such tree at load: a lambda a call returns
(`LET g = (MAKER ints)`, then `(g {k = one})`), and a member of a module a
functor built (`(b.g {k = one})` with `b = (BUILDER ints)`). Its call site
carries no trace, no slot map and no request, so the call reads its
arguments and computes its key from the values
([`a_callee_born_off_the_chain_computes_its_key_from_the_values`](../../src/dispatch/tests/laid.rs)).
Its parent, read off the callee's closure, is the layer of the frame that
built it, which the load laid where the builder's call is traced; the call
misses beneath it at its first call. That layer rests in a depot slot the
budget may evict, its types in the agent tier, and a refusal in it faults the
call where every traced binding's refuses the load. Tracing the site alone
would not have the load lay the layer: the load reads a callee's parent off
the site's lexical chain, and this callee is born in the builder's frame,
off that chain.

**Acceptance criteria.**

- The walk traces a call whose callee is a call's result, a returned `FN`
  and a built module's member alike, where the builder's call is traced: what
  the walk records of it names the callee's body and the layer it was born
  under, the body and binding of the frame that built it.
- Such a site carries a request, as a traced site does, and a call there
  computes no key.
- The load lays the callee's layer under each binding the trace names at
  such a call, beneath the builder's layer the load laid. The layer rests in
  program storage, its types in the frozen load, and its refusal refuses the
  load.
- A test: `(g {k = one})` after `LET g = (MAKER ints)`, and a functor's
  result bound at the top level and its member called there, each compute
  no key and miss nothing at run time.

**Directions.**

- *Where the builder is recorded — open.* On what the load knows of the
  value the builder returns — a module's closed content tree, a returned
  function's — so every reader of it sees the builder; or beside the call in
  what the walk records for the trace alone, since only the trace reads it.

## Dependencies

**Requires:** none.

**Unblocks:** none.
