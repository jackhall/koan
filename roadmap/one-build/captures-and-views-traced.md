# Captures and views traced

A slot followed through captures, `LET` aliases, member reads, views and
retypes by a call site's slot map.

**Problem.** A call site's slot map says what the site passes at each of its
callee's slots: load content, or a parameter of the site's own body passed on
bare, and that only at the callee's own closed declared type
([the trace](../../src/build/README.md#the-trace)). Anything else the text
builds from a slot passes nothing the trace follows — a capture of an
encloser's parameter, a `LET` bound to one, a `:|` or `:!` over one, a member
read out of one, a parameter passed bare at a closed declared type other than
its own, a module read back out of a record or union value a member holds, by
a field read or a `MATCH` arm's `it` — so a site passing one at a keyed name
carries no request
([references between frames and the depot](../../src/build/README.md#references-between-frames-and-the-depot)):
every call there reads the argument, digests it and computes the key from
the values. Each is a function of what the chain of layers binds: a capture's
content is its source's binding, an alias's its right side's, a member read's
the binding's member's, a field read's or an arm's `it`'s the part of the
value it reads, a view's its application and `from*` over its
underlying's
([carriers and paths](../../design/modules.md#carriers-and-paths)), and a
retype's the closed type it pins over its operand's bare digest
([content digests](../../src/values/README.md#content-digests)).

**Acceptance criteria.**

- A slot map names, at each keyed name, a **content tree**
  ([the load and the run](../../design/modules.md#the-load-and-the-run))
  whose every leaf is load content or a traced slot of a body on the site's
  lexical chain. The tree's nodes are the **narrowing** nodes, a view and a
  retype; the **passing** nodes, a `LET` alias read through to its right
  side, a capture, a pin in a view's application, a member chain read
  through a body, a field read, and a `MATCH` arm's `it` read through to its
  scrutinee — a variant's payload under `UNDER`, the scrutinee at its guard
  under `WITH`; and the **building** node, a record or variant built over
  content, which a field read or an arm's `it` reads back through. A tree
  holding a **wrapping** node — a body or a knot over a slot — is not such a
  tree: its site carries no request.
- A site whose keyed names each hold such a tree carries a request under
  every layer of its owner's body, laid from the chain's binding by one
  **tree lay**: a pure evaluation of the tree that reads the layer's ask, the
  chain's closed trees, and the signatures and member trees the records
  hold, and calls the walk's own doors, re-running no unit and standing no
  pass. The leaves take the binding; a member chain, a field read and an
  arm's `it` read the part's tree out of the binding's closed tree; a view
  node's `from*` comes from one *fits* solve over the binding's closed tree
  at its type; and the tree digests by the run's recipe, so a request a
  layer laid and one computed from the values reach one key. A view node
  that *fits* refuses, a member or field the value lacks, and an arm whose
  variant or guard the value misses lay no request for that site: the run
  refuses, or takes another arm, before the call.
- Each closed tree a load layer's request names rests in the load's index,
  where the callee's layer, a miss at run and a key computed from the values
  each find it. A settlement made at run lays a site's request only where the
  load or the callee's chain holds each tree it names.
- A slot map's owner is the innermost body that keys, from the site's body
  outward to the outermost body whose slot it passes. Its layer is the depot
  layer the site's frame runs under, so a call reads its request as it does
  today: one lookup by segment and site, through no parent.
- A body's traced slots include each slot a site's tree reaches, and the
  trace's fold names each binding as closed content through the same tree
  lay, so it names bindings through every node above, and
  [the trace](../../src/build/README.md#the-trace) says why a body's
  bindings are finite. Every binding is a part of load content or a view or
  retype over one: a member chain, a field read and an arm's `it` descend
  content, whose digests are well-founded; a view is its exposure over its
  underlying, a body-born module or a module a barrier re-viewed, since a
  view of a view that does not hide again what its source hides is refused;
  and a retype is a closed type over the bare digest. So a recursion over
  data ends where the data ends, and the fold needs neither the cap nor a
  cycle rule to end; the cap bounds only how many bindings a body names.
- Tests: a nested body passing its encloser's keyed parameter to a keyed
  callee; a body passing `(items :| Counter)` over its parameter `items`; a
  lambda passing its encloser's `view`, bound by
  `LET view = (items :| Counter)`; a parameter passed bare at a closed
  declared type other than its own; a view of a view; a view pinned at a
  `FOR ALL` name of the body; a `:!` over a parameter; a member read out of
  a parameter; and the recursion over a nominal recursive type in
  `a_signature_in_a_recursive_group_types_a_list_of_modules`
  ([`dispatch/tests/modules.rs`](../../src/dispatch/tests/modules.rs)), whose
  `(WALK it)` passes the module a member's union holds — each compute no key
  at run time.
- A body passing a member of its parameter back to itself, over a module
  nested two deep through an `Any` member, has its two bindings named by the
  fold, below the cap.
- The property laws hold unchanged
  ([properties](../../src/build/README.md#properties)).

**Directions.**

- *A content tree, not a path — decided.* The load already reads every
  module-valued expression as a content tree, digested by the run's own
  recipe, and a path from one slot is its one-leaf case. The tree takes a view
  pinned at another slot and a member chain for free, and records no order of
  steps: a view is canonical by its exposure, a retype is the last type over
  the bare digest, and a retype under a view decides only whether the view is
  admitted.
- *Narrowing and passing nodes followed, wrapping nodes not — decided.* A view
  or retype over a fixed leaf takes finitely many values; a body or knot over
  a slot is a code digest over the slot's content, which grows round a cycle
  of the call graph. Following them would need an SCC rule or the cap for
  termination, and their content is
  [callees built by a call traced](built-module-members.md)'s and
  [partial evaluation](partial-evaluation.md)'s ground.
- *A member chain over a leaf followed — decided.* The load cannot see into a
  parameter, but the tree lay holds the binding's closed tree and reads the
  member's tree out of it, to any depth through bodies; a list or a
  dictionary, a wrapping node or an unfollowed call ends it. A member whose application leaves a parameter open
  is read under the binding, which fixes it.
- *Modules in nominal values followed — decided.* A recursion over a nominal
  recursive type reaches its next module through a field or a variant's
  payload, since only a nominal member closes a type cycle; so a field read,
  an arm's `it` and the record or variant built over content are tree nodes.
  A module read out of a record or union *parameter* is
  [its own item](nominal-parameters-traced.md): such a parameter keys
  nothing.
- *Building and arm nodes in every tree — decided.* The walk builds them
  wherever it reads a record, a construction or an arm's `it`, not only where
  a slot map records one, and a body's member trees include each value binder
  whose tree is a building node. So the trace and the walk read one tree, and
  a path through `it` is classed by the scrutinee's tree. The alternative,
  rewriting an `it` leaf to its scrutinee only inside a slot map, gives the
  two of them different readings of `it`.
- *One tree lay, at the fold and at every layer — decided.* One pure function
  evaluates a tree under a binding; the fold calls it as it pushes and each
  layer as it lays. It reads only what the records hold, as a standing body's
  reads do, so a site whose unit a layer absorbed or read from a witness is
  laid as a walked one is. The alternatives — a forced re-run of the site's
  unit at each layer, a pass stood at the site, a fold naming derived content
  symbolically — cost a walk per layer, open a second way into the pass, or
  need the cap to end a recursion over data.
- *Residency on the load's index — decided.* The trees a load layer's
  requests name rest in the load's index. A settlement made at run keeps its
  own, so a callee born outside the owner's chain finds none there and
  decides in its activation, as such a call does today. Carrying the trees
  into the callee's settlement is depot code
  [the build over the binding graph](build-over-the-binding-graph.md)
  deletes.
- *Where the alias stops — decided.* A `LET` is followed where its right side
  is. One bound to a call's result — a functor's application, a returned `FN`
  — is not. One reading a `USING … SCOPE` surfaced name is not, since the load
  cannot type those from content
  ([frontiers and seams](../../src/build/README.md#frontiers-and-seams)).
- *A statement seam still keys nothing — decided.* The trace follows content,
  not facts: an alias's content is a function of its body's entry binding at
  every hop, so the body keys on the source slot, as it does for a parameter
  passed on bare, and the depot slot a run cell borrows never changes.
- *Ownership — decided.* The innermost keyed body owns, so the read stays one
  lookup; a capture-only site in a body with layers of its own has its request
  laid once per binding of that body beneath each encloser layer, bounded by
  the trace's cap. The alternative — the outermost source body owns and the
  read walks the chain — still needs this rule for a site passing both a
  capture and an own parameter. Measuring the duplication is
  [unplanned work](README.md#unplanned-work).

## Dependencies

Written against the trace and its slot maps. Planning restates it over the
binding graph, as what a `PassesName` mapping may write: a capture, a `LET`
alias, a member read, a view, a retype.

**Requires:**

- [The build over the binding graph](build-over-the-binding-graph.md) — the
  edges whose mappings this widens.

**Unblocks:**

- [A module read out of a nominal parameter traced](nominal-parameters-traced.md) — follows the tree past a record or union parameter.
