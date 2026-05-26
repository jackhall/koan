# Lexical provenance plumbing

Attach lexical position to every unit of work and to every block entry, so
later phases can ask "is this binding lexically before that reference?"
without changing resolution semantics yet.

**Problem.** Today a work item carries no record of *where in its enclosing
block it came from*. The driver pops nodes in FIFO order and re-derives any
notion of "earlier sibling" from the placeholder side-effect of submission.
There is no per-statement index, no per-block scope identity that
resolution can query, and no carrier on park/resume that survives a TCO
hop. Without provenance the index gate has nothing to gate on.

**Impact.**

- *Each unit of work carries its lexical chain.* An immutable `Rc`-linked
  frame `{ scope_id, index, parent }` rides on every node — shared across
  branches, copied across park/resume and TCO as a pointer. Concurrent
  siblings each carry their own.
- *One primitive enters a block.* `enter_block(scope_id, statements)`
  prepends a frame and dispatches each statement `i` with
  `(scope_id, i) :: parent`. Top-level, FN body, and MODULE body all flow
  through it (top-level is `enter_block` with an empty parent chain).
- *Inheritance is the default; prepending is the explicit action.* Chain
  propagation is centralized in the single node-creation path, so an
  omission is the safe (inherit) choice.
- *Lands as a pure refactor.* Resolution still uses the placeholder
  side-effect; the chain is carried but unread, so the phase is
  verifiable by an "every dispatched node has a non-empty chain past
  top-level" assertion before any semantics flip.

**Directions.**

- *Frame representation — decided.* Cactus-chain `Rc<Frame>`; immutable;
  prepend is the only mutation; per-node carrier defaults to inherit.

- *Single node-creation funnel — open.* Confirm every path that creates a
  `Dispatch` / sub-`Dispatch` / park-resume / TCO continuation routes
  through one place where the chain is attached.

- *Block boundaries — decided.* `MODULE` body is a new block (own
  `scope_id`, prepended frame). `USING … SCOPE` is a continuation of the
  enclosing block (no new frame — nothing binds into the transparent
  window; it only widens reads, see
  [`Scope::child_transparent`](../../src/machine/core/scope.rs)).
  Top-level, FN body, and MODULE body all enter through the same
  `enter_block` primitive.

- *Branch-arm blocks — open.* Whether each branch arm gets its own
  `scope_id` / index space or continues the enclosing block. Decision can
  ride with this phase or defer to
  [index-gated resolution](index-gated-resolution.md) — wherever the
  enter_block call site for arms lives.

- *Lexical-index assignment — open.* Where per-block statement indices
  attach (parse / lowering) and confirming the multi-statement body shape
  — top-level is `Vec<KExpression>`, but the multi-statement *body* shape
  isn't yet pinned. Each block is a statement sequence; the index space
  is per block.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Index-gated resolution](index-gated-resolution.md) — the gate reads
  the chain this phase plumbs.
