# Lexical-order name resolution

Make a name's *visibility* a function of its lexical position, not of the
scheduler's queue order — so forward references resolve (or fail) the same way
regardless of how sibling work is enqueued, reordered, or parallelized.

**Problem.** Whether a forward reference resolves today depends on submission
order, not lexical structure. A binder installs its placeholder as a side-effect
of being submitted ([`add` →
`extract_pre_run_name` → `install_placeholder`](../src/machine/execute/scheduler/submit.rs)),
and a consumer that dispatches and finds a `Resolution::Placeholder` parks on it,
while `Resolution::Unbound` is treated as a no-match / `UnboundName`
([`resolve_dispatch`](../src/machine/core/resolve_dispatch.rs)). That a consumer
sees `Placeholder` rather than `Unbound` holds *only* because the driver submits
every sibling in a block before `execute` pops the first node — a property of the
FIFO loop, not of the program. The single signal "does a placeholder exist yet"
conflates two distinct questions — *is this name in scope here?* (lexical) and
*has its value been computed yet?* (timing) — so dispatch behavior is coupled to
queue order, and reordering the queue or running siblings concurrently could make
an in-scope name read as `Unbound`.

**Impact.**

- *Forward references resolve by lexical position, deterministically.* A
  reference sees a binding iff the binding lexically precedes it under the block /
  scope rules, independent of which node the scheduler ran or installed first.
- *Sibling parallelism and queue reordering become safe.* Because visibility is
  decided from data carried on the work (lexical indices), not from arrival order,
  the scheduler is free to run a block's statements concurrently or in any order
  without changing which names resolve.
- *`UnboundName` becomes structural.* A name is unbound exactly when no binding is
  visible under the lexical rule — replacing the "every binder was submitted
  first" assumption the current `UnboundName` path rests on (see
  [design/typing/ktype.md § Dispatch and slot-specificity](../design/typing/ktype.md#dispatch-and-slot-specificity)).
- *Mutual recursion and forward function references keep working.* A body
  reference resolves when the body evaluates, against the enclosing scope as of the
  triggering use — so a function called after its mutually-recursive partner is
  defined still resolves it.
- *Visibility and readiness become separable concerns.* The index gate answers
  "in scope?"; the existing park edge answers "value ready?" — a visible-but-not-yet-computed
  binding parks on its producer as a genuine data-dependency edge.

**Directions.**

- *Visibility rule — decided.* A binding `D` at lexical index `i` in block `B` is
  visible to a reference `U` iff, walking `U`'s lexical (definition) scope chain to
  `B`, `i < cutoff(B)`. `cutoff(B)` is the lexical index of the statement-in-`B`
  on `U`'s call chain, or `∞` when `B` is not on the chain (already returned ⇒
  complete). An immediate sibling reference is gated by its own statement's index
  (sees earlier siblings only); a deferred body, on evaluation, sees an enclosing
  block up to the index of the call that triggered it, and sees a fully-returned
  enclosing block in its entirety.

- *Returned-block locals are visible to deferred bodies — decided.* When an
  enclosing block has fully returned before a deferred body runs, that block's
  later-defined locals are visible (the `∞`/complete case above). Uniform with the
  deferred-body principle that already makes top-level mutual recursion work;
  rejects the capture-at-definition (ML `letrec`) alternative.

- *Cutoff is a fixed per-statement index, not a mutable frontier — decided.* The
  cutoff for a block is the lexical index of the specific statement on this
  resolution's call chain, not the block's running execution progress (which is
  ill-defined when siblings run concurrently). Each unit of work carries its own
  lexical provenance: an immutable per-work-item chain of `(block scope_id, lexical
  index)` frames, queried by block identity during the `outer` walk. Two sibling
  statements evaluating concurrently each carry their own index and see their own
  cutoff.

- *Split the placeholder's two roles — decided.* Visibility moves to the index
  gate; the [park edge](../src/machine/execute/scheduler/dispatch.rs) is retained
  solely for readiness — a visible binding whose producer hasn't finished computing
  parks on it. `Resolution` distinguishes "not visible here" (→ `UnboundName`) from
  "visible, not ready" (→ park).

- *Provenance representation — decided.* A cactus-chain: an immutable, `Rc`-linked
  frame `{ scope_id, index, parent }`, shared across branches (carrying it across
  park/resume and TCO is a pointer copy) — not a mutable call stack, since
  concurrent siblings each need their own cutoff. Inheritance is the default and
  prepending is the only explicit action, so an omission is always the safe
  (inherit) choice: chain propagation is centralized in the single node-creation
  path, and exactly one `enter_block(scope_id, statements)` primitive prepends a
  frame (dispatching each statement `i` with `(scope_id, i) :: parent`). Resolution
  walks the [`Scope`](../src/machine/core/scope.rs) `outer` chain and, per block,
  looks up its `scope_id` in the provenance chain — hit ⇒ that index, miss ⇒ `∞`.

- *Block boundaries — decided.* A `MODULE` body is a new lexical block (own
  `scope_id`, prepended frame). `USING … SCOPE` is a continuation of the enclosing
  block (no new frame — nothing binds into the transparent window; it only widens
  reads, see [`Scope::child_transparent`](../src/machine/core/scope.rs)). Top-level,
  FN body, and MODULE body all enter through the same `enter_block` primitive
  (top-level is `enter_block` with an empty parent chain).

- *Branch-arm blocks — open.* Whether each branch arm is its own lexical block (own
  `scope_id` / index space) or a continuation of the enclosing block.

- *Lexical-index assignment — open.* Where per-block statement indices are attached
  (parse / lowering), and confirming the multi-statement block representation —
  top-level is `Vec<KExpression>`, but the multi-statement *body* shape isn't yet
  pinned. Each block is a statement sequence; the index space is per block.

- *Transparent and forwarding scopes — open.* How the gate composes with
  `USING … SCOPE` transparent windows and module-body scopes, whose reads and
  writes already split across `outer` (see
  [`Scope::child_transparent`](../src/machine/core/scope.rs) and
  [design/typing/modules.md](../design/typing/modules.md)).

## Dependencies

No roadmap-level prerequisites — the substrate it reshapes is all shipped: the
placeholder / park machinery in
[`resolve_dispatch`](../src/machine/core/resolve_dispatch.rs),
[`submit`](../src/machine/execute/scheduler/submit.rs), and
[`dispatch`](../src/machine/execute/scheduler/dispatch.rs), and the lexical scope
chain in [`Scope`](../src/machine/core/scope.rs) (whose per-call child's `outer` is
already the captured definition scope, the static link this rule walks). The
[execution model](../design/execution-model.md) describes the dispatch / park
pipeline this changes.

This unblocks parallel or reordered scheduling of sibling work, which is not yet
its own roadmap item; resolution becoming queue-order-independent is the
prerequisite that work would build on.
