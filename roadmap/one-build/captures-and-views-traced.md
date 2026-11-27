# A module passed through an alias, a capture, a view or a member read

A call's `PassesName` mapping written through captures, `LET` aliases, member
reads, field reads, views and retypes, so the load makes the callee's
instance.

**Problem.** A call's mapping gives its callee's module names load content,
or a parameter of the caller's own body passed on bare (`Passing::Fixed` in
[`site.rs`](../../src/scope/site.rs), `slots_mapped` in
[`lay.rs`](../../src/build/lay.rs)). Anything else the text builds from a
parameter maps nothing: a capture of an encloser's parameter, a `LET` bound
to one, a `:|` or `:!` over one, a member read out of one, a field read or a
`MATCH` arm's `it` over a value holding one. The callee's instance then waits
for a run cell, and a refusal the text decides under the caller's binding
faults at run where a bare pass refuses the load
([refusal](../../src/build/README.md#refusal)). Under

```koan
SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
MODULE ints = ((LET zero = 0))
MODULE strs = ((LET zero = "a"))
EXPR #(NEEDS n :Number) -> Number = #(n)
EXPR #(USE m :Counter) -> Any = #(NEEDS m.zero)
PRINT "start"
```

`USE strs` refuses, since `strs.zero` is a `Str`. This refuses the load,
naming `(USE g)`:

```koan
EXPR #(VIA g :Counter) -> Any = #(USE g)
PRINT (VIA strs)
```

Each of these prints `start` and then faults:

```koan
EXPR #(VIA g :Counter) -> Any = #(
  (LET h = g)
  (USE h)
)
PRINT (VIA strs)
```

```koan
EXPR #(VIA g :Counter) -> Any = #(
  (LET f = (FN :{k :Number} -> Any = #(USE g)))
  (f {k = 1})
)
PRINT (VIA strs)
```

```koan
EXPR #(VIA g :Counter) -> Any = #(USE (g :| Counter))
PRINT (VIA strs)
```

```koan
EXPR #(VIA g :Counter) -> Any = #(USE (g :! Counter))
PRINT (VIA strs)
```

```koan
SIG Outer = #[(VAL inner :Counter)]
MODULE outer = ((LET inner = strs))
EXPR #(VIA o :Outer) -> Any = #(USE o.inner)
PRINT (VIA outer)
```

At the top level, where each is load content, `LET alias = strs` then
`USE alias`, and `USE (strs :| Counter)`, refuse the load.

**Acceptance criteria.**

- A call's mapping gives a callee's module names a term over the caller's
  names, a **content tree**
  ([the load and the run](../../design/modules.md#the-load-and-the-run))
  whose every leaf is load content or a name of a body on the site's lexical
  chain. Its nodes are the **narrowing** nodes, a view and a retype; the
  **passing** nodes, a `LET` alias read through to its right side, a
  capture, a pin in a view's application, a member chain read through a
  body, a field read, and a `MATCH` arm's `it` read through to its
  scrutinee; and the **building** node, a record or variant built over
  content, which a field read or an arm's `it` reads back through.
- Applying such a term to a binding
  ([tree application](../../src/dispatch/graph/README.md#the-domain))
  gives the content a run binding the callee's parameter would bind, digest
  for digest, so the instance the load makes is the one the run cell finds.
  A view that *fits* refuses, a member or field the value lacks, and an arm
  the value misses give no content: the run refuses, or takes another arm,
  before the call.
- Each of the five faulting programs above refuses the load and prints
  nothing.
- Tests, each asserting the load makes the callee's instance: a nested body
  passing its encloser's module parameter; a body passing
  `(items :| Counter)` over its parameter `items`; a lambda passing its
  encloser's `view`, bound by `LET view = (items :| Counter)`; a parameter
  passed bare at a closed declared type other than its own; a view of a
  view; a view pinned at a `FOR ALL` name of the body; a `:!` over a
  parameter; a member read out of a parameter; and the recursion over a
  nominal recursive type in
  `a_signature_in_a_recursive_group_types_a_list_of_modules`
  ([`dispatch/tests/modules.rs`](../../src/dispatch/tests/modules.rs)), whose
  `(WALK it)` passes the module a member's union holds.
- A body passing a member of its parameter back to itself, over a module
  nested two deep through an `Any` member, builds: laying the graph does not
  refuse it as a recursion that grows
  ([refusing a recursion that grows](../../bindgraph/README.md#refusing-a-recursion-that-grows)),
  since a member chain descends content.
- A `:|` the view rule does not type, over a tree no binder roots, such as
  `(it :| Counter)` over a parameter's part, is typed by a derived name, and
  under a binding that closes its operand it is the view a fresh load with
  the binding declared builds. A test checks both at a meet no shown
  application spells.
- The property laws hold unchanged
  ([properties](../../src/build/README.md#properties)).

**Directions.**

- *A `:|` the view rule does not type — decided.* A derived-name term
  `DerivedTerm::View { tree, application, names }`: the view at the closed
  `application` of the module whose content tree is the source of the view
  tree of identity `tree`, which reads through `names`, the unknowns a mark
  on it marks. Its ends are `Never` up to the empty signature, and
  `each_type` visits the application. The walk types such a `:|` by laying
  that name, except over a tree no binding can substitute (an `Unnamed`
  leaf), where the meet-rule narrowing stays. A binding closes it through a
  `Binds::view`: the tree the binding substitutes for `tree`; where that
  tree is closed, the view a fresh load builds over it, and where it stays
  open, the name of the view over the tree it leaves. `bound_derived` hands
  a `View` term to `Binds::view` rather than substituting its types.
- *A content tree, not a path — decided.* The load reads every
  module-valued expression as a content tree, digested by the run's own
  recipe, and a path from one parameter is its one-leaf case. The tree takes
  a view pinned at another parameter and a member chain for free, and
  records no order of steps: a view is canonical by its exposure, a retype
  is the last type over the bare digest, and a retype under a view decides
  only whether the view is admitted.
- *Narrowing and passing nodes followed, wrapping nodes not — decided.* A
  view or retype over a fixed leaf takes finitely many values; a body or
  knot over a parameter is a code digest over the parameter's content, which
  grows round a cycle of the call graph. Their content is
  [a callee read by a member path or a call's result](callees-read-by-path.md)'s
  and [partial evaluation](partial-evaluation.md)'s ground.
- *A member chain over a leaf followed — decided.* The load cannot see into
  a parameter, but tree application holds the binding's closed tree and
  reads the member's tree out of it, to any depth through bodies; a list or
  a dictionary, a wrapping node or an unfollowed call ends it.
- *Modules in nominal values followed — decided.* A recursion over a nominal
  recursive type reaches its next module through a field or a variant's
  payload, since only a nominal member closes a type cycle; so a field read,
  an arm's `it` and the record or variant built over content are tree nodes.
  A module read out of a record or union *parameter* is
  [its own item](nominal-parameters-traced.md): such a parameter binds no
  name.
- *Where the alias stops — decided.* A `LET` is followed where its right
  side is. One reading a `USING … SCOPE` surfaced name is not, since the
  load cannot type those from content
  ([frontiers and seams](../../src/build/README.md#frontiers-and-seams)). One
  bound to a call's result is a holder, which an onward call instances
  ([the binding graph's text](../../src/build/README.md#the-binding-graphs-text)).

## Dependencies

**Requires:** none.

**Unblocks:**

- [A module read out of a nominal parameter](nominal-parameters-traced.md) — follows the tree past a record or union parameter.
