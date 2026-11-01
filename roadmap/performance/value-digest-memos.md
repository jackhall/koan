# A digest memo on every value

**Optional.** No program outside a test shows the shape it pays for.

**Problem.** A content digest is kept only by a module, as its content, and by
a slot the load marks as digested
([content digests](../../src/values/README.md#content-digests)); every other
demand walks what the value reaches through a memo that lives one demand. A
loop that builds a module each hop over a value that grows with the loop —
`MODULE m OVER #[link]` over a chain of `{next :Any}` records — and views or
keys it walks the rest of the chain each hop, since each hop's module is new
and the chain's own parts keep nothing: quadratic in the loop's length. The
caching suite's per-hop digest test
([caching.rs](../../src/dispatch/tests/caching.rs),
`a_hops_digest_visits_hold_flat_or_pay_for_the_module_they_view`) names five
such programs, each paying one walk of the rest of the chain per hop.

**Acceptance criteria.**

- A loop that views or keys a new module each hop over the rest of a chain
  digests each link of the chain a bounded number of times over the whole
  loop: in the per-hop digest test every program's visits per hop are flat in
  depth.

**Directions.**

- *Where the memo sits — open.* On every composite header and knot root, at
  16 B per header on every value, or only on the values a load-marked slot or
  module capture reaches, which keeps a value no demand reaches at its size.
  Recommended: wait for a program outside a test that walks a chain this way,
  and size the choice by it.

## Dependencies

**Requires:** none.

**Unblocks:** none.
