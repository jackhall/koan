# An opaque view nested in a module, laid at load

**Problem.** A view over a module whose slot holds an opaque view interns
types at run, though the load sees the nested view's content tree. In

```
SIG Inner FOR ALL #[Elem] = #[(VAL item :Elem)]
SIG Outer FOR ALL #[Carrier] = #[(VAL zero :Carrier) (VAL inner :(Inner WITH {Elem = Carrier}))]
MODULE a = ((LET Elem = Number) (LET item = 5))
LET o = (a :| Inner)
MODULE ints = ((LET zero = o.item) (LET inner = o))
LET m = (ints :| Outer)
```

every tree is closed: `o` is a view of a body-born module, and `ints`
captures only `o`. Yet the run interns 11 types into its branch building `m`.
The same program with `o` held in a list (`(LET inners = [o])` under
`(VAL inners :(LIST OF (Inner WITH {Elem = Carrier})))`) interns 14. A body
held in either place interns none
([`dispatch/tests/views.rs`](../../src/dispatch/tests/views.rs)). Per
[where each body interns](../../design/type-tiers.md#where-each-body-interns),
the load computes every type the door computes from types alone over a closed
tree, so the run should mint nothing here.

The nested slot's pin names `Outer`'s carrier, which hides `o`'s own carrier
over `ints`. The run's [exposure](../../src/knot/module/README.md#the-view-door)
reads that pin back through `o`'s sealing and builds over `a`. The load lays
the nested view either through the tree's exposure or, where every parameter
is pinned, through `view::pinned_view`, which takes `from*` from the pins as
written ([`walk/views.rs`](../../src/dispatch/walk/views.rs) `nested_view`).
Which of the 11 types the load misses, and why, is not yet known.

The load reads nothing through a view's tree either. A type path through a
view's nested module member, `v.inner.Extra`, resolves by class over a leaf on
`v` whose chain `[inner]` nothing binds, so a rigid reaches the run, where
`LET i = v.inner` then `i.Extra` names the member. `inner`'s declared type
leaves `Extra` hidden; only the nested view carries it.

**Acceptance criteria.**

- The program above interns no type at run, and neither does its list form.
  Tests in `dispatch/tests/views.rs` pin both counts at 0.
- The nested view's `from*` and `hidden` that the load lays are those the run's
  exposure builds over `o`'s underlying.
- A type path through a view's nested module member names the member the
  nested view hides: over the `NESTED` prelude of
  [`dispatch/tests/modules.rs`](../../src/dispatch/tests/modules.rs),
  `PRINT v.inner.Extra` prints `Extra`, and `LET t :(v.inner.Extra) =
  v.inner.two` then `PRINT t` prints `Extra(s)`, as they do through
  `LET i = v.inner`. The test
  `a_path_through_a_nested_view_names_its_hidden_member` passes un-ignored.

**Directions.**

- *Where the load's nested lay diverges from the run's — open.* First
  attribute the 11 types: the outer view, the nested view's substitutions, or
  the coerced members. Then either read the pins back through the nested
  tree's sealing before `pinned_view` takes them, or have the tree's exposure
  answer for an opaque cell.
- *A path through a nested view — decided.* The load reads a member through a
  view's tree: a step through a `Tree::View` yields the nested member's view
  tree — the underlying's member viewed at its declared application with the
  outer view's hidden sides, as `nested_view` builds it — its digest the run's
  nested view's ([`graph/trees.rs`](../../src/dispatch/graph/trees.rs)
  `read_step`).

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
