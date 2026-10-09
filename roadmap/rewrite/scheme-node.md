# A scheme node

**Problem.** A quantified callable's type is a `KFunction` or an
`ExpressionShape` whose `quantifiers` are non-empty
([node.rs](../../lattice/src/types/node.rs)). Both variants carry a group, its
names and bounds, so `KFunction` is 80 B where its parameters and return need
48 B. An empty group binds nothing, and a `Quantified` under it reads the
enclosing binder, so whether a node binds is a question asked at run time
(`TypeNode::binds_quantifiers`): at each walk step, in the flag fold, in the
binary walk's function and shape arms, and in `instantiate_quantified`, which
has an arm per callable kind. A `Scheme` handle names one of the two variants,
and about fifteen readers take `TypeRegistry::scheme_node` or
`DeclaredType::node` and match the node as a function or a shape, five of them
behind `let … else unreachable!`.

**Acceptance criteria.**

- `TypeNode::Scheme { group, body }` is the one binder. `group` is one `&'run`
  reference to the quantifier names and bounds; `body` is a function type or an
  expression shape whose free `Quantified` positions read that group.
  `KFunction` and `ExpressionShape` carry no group.
- A `Scheme` handle names a `Scheme` node. `function_scheme` and `shape_scheme`
  are the only doors that mint one, over a non-empty group, and answer the
  plain type for an empty one.
- A scheme's digest is its tag, its group's arity and bounds, and its body's
  digest; a function type's and a shape's digests take no group. Two schemes
  that differ only in their quantifier names are one handle, pinned by a law.
- `TypeRegistry::scheme_node` and `DeclaredType::node` do not exist. A reader of
  a declared callable gets its group and its body's parts through one view.
- Instantiating a scheme substitutes its body, with no arm per callable kind.
- The binary walk relates a scheme to anything at the leaf verdict, and its
  function and shape arms read no group.
- `TypeNode::binds_quantifiers` is true of a `Scheme` alone, and the walk
  drivers count binder depth there.
- The type lattice's design docs describe the scheme node: the
  [vocabulary](../../lattice/src/types/vocabulary.md),
  [identity](../../lattice/src/types/identity.md),
  [relations](../../lattice/src/types/relations.md) and the walk section of its
  [README](../../lattice/src/types/README.md).
- The property suite passes, its generators drawing every scheme through the
  scheme doors.

**Directions.**

- *One variant for both callables — decided.* The body's node says whether a
  scheme is a function or a shape. Every reader that branches on that also
  reads the body's parts, so separate `FunctionScheme` and `ShapeScheme`
  variants would save no lookup.
- *The group behind one reference — decided.* `Group { names, bounds }` is laid
  on an intern miss, so `Scheme` fits the
  [compact node table](compact-type-node-table.md)'s 48 B node with its body
  inline. A reader of the group alone, such as a solve reading bounds or a
  refusal naming a variable, never reads the body.
- *Readers through one view — decided.* Deleting `scheme_node` and
  `DeclaredType::node` makes every reader that matched a scheme's node a
  compile error until it reads the view, which holds the one
  `let … else unreachable!` resting on the scheme doors' invariant.
- *Order and fits — decided.* A pair holding a scheme reads both bodies:
  `admits_function` for two function bodies, `admits_shape` for two shapes,
  and no fit across kinds. In the order a scheme lies under itself alone, and
  `family_top` puts it in the value family.

## Dependencies

**Requires:** none.

**Unblocks:**

- [A compact type node table](compact-type-node-table.md) — its `KFunction`
  and `ExpressionShape` fit 48 B because they carry no group.
