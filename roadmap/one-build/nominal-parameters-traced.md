# A module read out of a nominal parameter traced

A slot map followed past a record or union parameter to the module it holds.

**Problem.** Under [captures and views traced](captures-and-views-traced.md) a
content tree's leaf is load content or a traced slot, and only a module or a
type value keys
([keys](../../src/build/README.md#keys-which-bindings-make-a-settlement)): a
record or union parameter keys nothing, since its digest costs its size, and a
record value is not load content. So a site that reads a module out of such a
parameter is untraced: it computes its key from the values, and its callee is
open. In this program, which prints `14`, `(CARRIER xs.head)` is such a site:

```koan
SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]
MODULE ints = ((LET zero = 0) (LET succ = (FN :{x :Number} -> Number = #(x + 1))))
NEWTYPE Chain = :{head :Counter, tail :(Chain | Null)}
EXPR #(CARRIER m :Counter) -> Number = #((LET v = (m :| Counter)) (7))
EXPR #(COUNT xs :Null) -> Number = #(0)
EXPR #(COUNT xs :Chain) -> Number = #((CARRIER xs.head) + (COUNT xs.tail))
LET two = (Chain {head = ints, tail = (Chain {head = ints, tail = null})})
PRINT (COUNT two)
```

**Acceptance criteria.**

- In the program above, `(CARRIER xs.head)` computes no key at run time, and
  `CARRIER` stays traced for its other callers.
- The property laws hold unchanged
  ([properties](../../src/build/README.md#properties)).

**Directions.**

- *What the parameter's binding is — open.* A record or union parameter keys
  on the content of the module parts its sites read, a projection rather than
  its whole digest; or a record or union value built over load content
  becomes load content, keyed by its digest. Recommended: the projection,
  since it keys the body only on what the trace follows.

## Dependencies

**Requires:**

- [Captures and views traced](captures-and-views-traced.md) — the content tree and the tree lay this extends.

**Unblocks:** none.
