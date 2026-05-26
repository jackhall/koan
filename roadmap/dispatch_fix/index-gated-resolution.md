# Index-gated resolution

Flip name resolution from "does a placeholder exist yet?" to
"is this binding visible from this reference's lexical position?". Split
the `Resolution` outcome accordingly, so visibility (lexical) and
readiness (timing) are separately answerable.

**Problem.** A binder installs its placeholder as a side-effect of being
submitted ([`add` → `extract_pre_run_name` →
`install_placeholder`](../../src/machine/execute/scheduler/submit.rs)),
and a consumer that dispatches and finds a `Resolution::Placeholder`
parks on it, while `Resolution::Unbound` is treated as a no-match
([`resolve_dispatch`](../../src/machine/core/resolve_dispatch.rs)). That
a consumer sees `Placeholder` rather than `Unbound` holds *only* because
the driver submits every sibling in a block before `execute` pops the
first node. The single placeholder signal conflates *is this name in
scope here?* (lexical) and *has its value been computed yet?* (timing)
— so dispatch behavior is coupled to queue order, and reordering or
running siblings concurrently could make an in-scope name read as
`Unbound`.

**Impact.**

- *Forward references resolve by lexical position, deterministically.* A
  reference sees a binding iff the binding lexically precedes it under
  the block / scope rules, independent of submission order.
- *Sibling parallelism and queue reordering become safe.* Visibility is
  decided from data carried on the work, not from arrival order.
- *`UnboundName` becomes structural.* A name is unbound exactly when no
  binding is visible under the lexical rule — replacing the "every binder
  was submitted first" assumption (see
  [design/typing/ktype.md § Dispatch and slot-specificity](../../design/typing/ktype.md#dispatch-and-slot-specificity)).
- *Visibility and readiness become separable.* The index gate answers
  "in scope?"; the existing park edge answers "value ready?" — a
  visible-but-not-yet-computed binding parks on its producer as a genuine
  data-dependency edge.
- *Mutual recursion and forward function references keep working.* A body
  reference resolves when the body evaluates, against the enclosing scope
  as of the triggering use.

**Directions.**

- *Visibility rule — decided.* A binding `D` at lexical index `i` in
  block `B` is visible to a reference `U` iff, walking `U`'s lexical
  scope chain to `B`, `i < cutoff(B)`. `cutoff(B)` is the lexical index
  of the statement-in-`B` on `U`'s call chain, or `∞` when `B` is not on
  the chain (already returned ⇒ complete). An immediate sibling reference
  is gated by its own statement's index (sees earlier siblings only); a
  deferred body, on evaluation, sees an enclosing block up to the index
  of the call that triggered it, and sees a fully-returned enclosing
  block in its entirety.

- *Returned-block locals are visible to deferred bodies — decided.* When
  an enclosing block has fully returned before a deferred body runs,
  that block's later-defined locals are visible (the `∞`/complete case
  above). Uniform with the deferred-body principle that already makes
  top-level mutual recursion work; rejects the capture-at-definition
  (ML `letrec`) alternative.

- *Cutoff is a fixed per-statement index, not a mutable frontier —
  decided.* The cutoff for a block is the lexical index of the specific
  statement on this resolution's call chain, not the block's running
  execution progress (which is ill-defined when siblings run
  concurrently). Two sibling statements evaluating concurrently each
  carry their own index and see their own cutoff.

- *Split the placeholder's two roles — decided.* Visibility moves to the
  index gate; the [park edge](../../src/machine/execute/scheduler/dispatch.rs)
  is retained solely for readiness — a visible binding whose producer
  hasn't finished computing parks on it. `Resolution` distinguishes
  "not visible here" (→ `UnboundName`) from "visible, not ready"
  (→ park).

- *Resolution walks `outer` and queries the chain — decided.* Per
  enclosing `Scope` on the [`outer`](../../src/machine/core/scope.rs)
  chain, look up its `scope_id` in the provenance chain — hit ⇒ that
  index, miss ⇒ `∞`. The chain is already plumbed onto every node;
  [`LexicalFrame::index_for`](../../src/machine/core/lexical_frame.rs)
  is the read-side hook to call (currently unread).

- *Transparent and forwarding scopes — open.* How the gate composes
  with `USING … SCOPE` transparent windows and module-body scopes,
  whose reads and writes already split across `outer` (see
  [`Scope::child_transparent`](../../src/machine/core/scope.rs) and
  [design/typing/modules.md](../../design/typing/modules.md)).

## Dependencies

**Requires:** none.

**Unblocks:**

- [Nested-binder recursive submission](nested-binder-submission.md) —
  the race that phase closes is only observable under strict-only
  admission, which in turn needs the structural `Placeholder` vs
  `Unbound` split this phase produces.
- [Unified walk + strict-only admission](unified-walk.md) — strict-only
  admission's `Placeholder` vs `Unbound` branch needs the structural
  split.

Also unblocks parallel or reordered scheduling of sibling work, which is
not yet its own roadmap item; resolution becoming queue-order-independent
is the prerequisite that work would build on.
