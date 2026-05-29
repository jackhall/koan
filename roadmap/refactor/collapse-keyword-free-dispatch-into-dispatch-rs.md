# Collapse all keyword-free dispatch into `dispatch.rs`

Replace the `value_pass` builtin with a `LiteralPassThrough` fast-lane
shape, move the struct/tagged-union constructors out of `builtins`
(they're dispatch-shape implementations, not user-facing builtins),
flatten the `dispatch::single_poll` wrapper module, and split the
resulting `dispatch.rs` into per-shape sibling files so the absorbed
content lives below the size-charge knee. Establishes the invariant
**keyword-free ⟺ Keyworded shape**, currently one-directional because
`value_pass` leaks literals into `Keyworded`.

**Problem.** The dispatcher classifies expressions into a `Keyworded`
catch-all plus five no-keyword fast-lane shapes (see
[`classify_dispatch_shape`](../../src/machine/execute/scheduler/dispatch.rs)).
The keyword-absence gate is the principal discriminator: every
keyword-bearing expression goes through `Keyworded`, and most
keyword-free ones go through a fast lane (`BareIdentifier`,
`BareTypeLeaf`, `ConstructorCall`, `FunctionValueCall`,
`SigiledTypeExpr`). Three legacy details leak this clean partition:

1. **Single-part literals fall through to `Keyworded`** because no
   fast-lane shape covers them. The `value_pass` builtin
   ([`src/builtins/value_pass.rs`](../../src/builtins/value_pass.rs))
   exists purely to give the dispatcher a target for `(99)`,
   `("x")`, `([1 2 3])`, etc., paying the full bucket-lookup +
   argument-bundling + builtin-call pipeline for what is conceptually
   a single arena re-allocation.
2. **Constructor builtins register dead targets.** `struct_construct`
   ([`src/builtins/struct_value.rs`](../../src/builtins/struct_value.rs))
   and `tagged_union_construct`
   ([`src/builtins/tagged_union.rs`](../../src/builtins/tagged_union.rs))
   are registered with `register_builtin`, but the `ConstructorCall`
   fast lane calls `struct_value::apply` / `tagged_union::apply`
   directly without any name lookup. The `register` and
   `primitive_body` functions plus the registered names
   (`"struct_construct"` / `"tagged_union_construct"`) are referenced
   only by their own registrations — dead code.
3. **The five existing fast-lane states live in a wrapper module.**
   [`src/machine/execute/scheduler/dispatch/single_poll.rs`](../../src/machine/execute/scheduler/dispatch/single_poll.rs)
   holds `BareIdState`, `BareTypeState`, `CtorState`, `SigilState`
   plus their `bare_identifier` / `bare_type_leaf` / `sigiled_type_expr`
   / `constructor_call` / `schedule_constructor_body` functions. The
   wrapper adds a depth-6 module whose only contents are the fast-lane
   bodies that conceptually belong next to `classify_dispatch_shape`.

The three are interlocked: collapsing `single_poll` makes
`dispatch.rs` the natural home for the new `LiteralPassThrough` state;
moving the constructors out of `builtins` only nets out structurally
once their destination isn't deep under `single_poll`; the
`value_pass` deletion is the smallest piece but it's what establishes
the keywords⟺Keyworded equivalence. The collapse alone grows
`dispatch.rs` past the per-file size-charge knee, so the final step
splits the absorbed content into per-shape sibling files (one file
per fast-lane shape, no new wrapper module).

**Impact.**

- `Keyworded` becomes a precise shape, not a misnomer: every
  Keyworded dispatch has at least one keyword. The fast-lane axis
  is exhaustive over keyword-free expressions.
- Four whole files retire (`value_pass.rs`, `struct_value.rs`,
  `tagged_union.rs`, `single_poll.rs`) and the constructor
  implementations land in a new `dispatch::constructors` peer
  module next to `dispatch.rs`.
- Builtins module count drops by three; the dispatch subtree
  flattens by one wrapper layer.
- Runtime dispatch on parens-wrapped literals skips bucket lookup +
  argument bundling + builtin call. Constructor dispatch loses one
  layer of indirection.
- Each fast-lane shape becomes a small self-contained file: a
  reader investigating `BareIdentifier` dispatch loads ~80 lines of
  one state plus its poll body, not a 700-line dispatch module.

**Scoring.** Measured via `modgraph` against the post-Pass-14
baseline (machine 218.98, crate 228.80) with `--reference-loc`
fixing the denominator and `--delete` / `--delete-file` modelling
the deletions:

| Sub-piece | machine Δ | crate Δ |
|---|---|---|
| Flatten `single_poll` alone | +0.25 | −3.01 |
| Delete `value_pass` + relocate constructors alone | +2.43 | +0.61 |
| **Collapse bundle (all three together)** | **+3.12** | **−9.49** |

The bundle is multiplicative, not additive — each piece enables the
others. `single_poll` flattening removes the wrapper depth that
made the constructor relocation costly; the deletions and
relocations materialize the crate-level coupling drop (−9.20). The
machine-root regression (+3.12) is the proximate motivation for the
final split step, which the metric expects to bring machine-root Δ
back to ≤ 0 without re-introducing the wrapper.

**Directions.**

- **Constructors land at `dispatch::constructors` peer, not merged
  into `dispatch.rs` — decided.** The peer variant scored crate
  Δ −9.49 vs the merged variant's −8.84; the peer also keeps
  `dispatch.rs` smaller after the absorption.
- **`LiteralPassThrough` state lives inline in `dispatch.rs`,
  joining `LitState` to the existing per-shape states — decided.**
  Same shape as the four existing single-poll states after the
  flattening; no new module needed.
- **Per-shape file granularity for the split — open.** Either one
  file per shape (six files: `bare_identifier`, `bare_type_leaf`,
  `constructor_call`, `sigiled_type_expr`, `function_value_call`,
  `literal_pass_through`) *or* one file per dispatch *category*
  (three files: `name_lookup`, `value_construction`,
  `type_expression`). Recommended: per-shape granularity, since
  each shape has its own state type and poll body and there are no
  reusable helpers between shapes.
- **Where `function_value_call` lives — deferred.** That shape's
  state is currently in `fn_value.rs`, separate from `single_poll`.
  After the collapse, it can stay where it is or move into the new
  per-shape layout for uniformity. Decide alongside the granularity
  choice; not a blocker.
- **Re-score after split — decided.** Run `modgraph` with
  `--reference-loc` against the post-collapse baseline and confirm
  machine-root Δ ≤ 0. If the split doesn't recover the bundle's
  +3.12 penalty, reconsider the granularity.

## Dependencies

**Requires:** none. All four target files (`value_pass.rs`,
`struct_value.rs`, `tagged_union.rs`, `single_poll.rs`) are
self-contained at the module level and only reach into stable
substrates (arena, scope, ktype, dispatch state). The
`ConstructorCall` fast lane already calls `apply` directly — no
name-lookup contract to preserve.

**Unblocks:** none.
