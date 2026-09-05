# Declaration windows gate dispatch resolution

**Problem.** A dispatch resolves against the scope chain as it stands when the
dispatch runs:
[`Scope::resolve_dispatch`](../../src/machine/execute/decide/resolve_dispatch.rs)
walks visible scopes innermost-first and reads each one's live
[`FunctionLookup`](../../src/machine/core/bindings.rs). Nothing gates that walk by
the lexical position of the body it runs inside — the type channel's
`idx < cutoff` binder-position gate
([design/typing/elaboration.md](../../design/typing/elaboration.md)) has no
value-channel counterpart for overload buckets.

Today the gap is inert. `EVAL` is frame-local
([`eval.rs`](../../src/builtins/eval.rs)), so no declaration can arrive after the
parse-static scan that seeds pending-overload claims, and every overload a bucket
will ever hold is one a sibling's claim already announced. Forward references
within a block work because `Bindings::install_pending_overload` records those
claims at statement submission, which is a declaration window in effect — but not
one the language names, and not one anything distinguishes a *later-arriving*
declaration from.

[EVAL splices in place](eval-splices-in-place.md) creates that distinction and the
gap stops being inert. A spliced declaration is computed, so the parse-static scan
cannot see it and it installs no claim; the barrier's dependency edges run from
later siblings to the barrier only. An earlier sibling therefore runs concurrently
with the splice, and any dispatch nested in a body it calls resolves against
whichever table it happens to observe. The same holds for a body declared before
the barrier and called after it, which observes the post-splice table through the
ordinary scope chain. `design/metaprogramming.md`'s rule that expressions before
the barrier never see the splice's bindings would hold at statement level and fail
for dispatches nested inside bodies.

**Acceptance criteria.**

- A block's top-level `FN`, `EXPR` and `OP` declarations form one window, scanned
  off the statement split, and every body in the block resolves against the whole
  set regardless of declaration order — a mutually recursive pair written in
  either order resolves without a forward declaration.
- A dispatch inside a body resolves against that body's window plus enclosing
  declarations bound before the body's binder position, and against nothing that
  lands later. A test that declares an `FN` before an `EVAL` barrier which splices
  a more specific overload its body would otherwise match observes the pre-splice
  resolution under every scheduler order — whether the `FN` is called before or
  after the barrier.
- An `EVAL`'s spliced top-level declarations form their own window, sealed when
  the splice finalizes: a splice declaring an `OP` together with an `EXPR` whose
  body uses it resolves the pair, with no ordering constraint between them inside
  the splice.
- A recursive cycle split across the boundary is refused rather than silently
  resolved: a literal declaration in a block and a spliced declaration cannot
  reference each other, and the diagnostic names the window rule.
- A spliced `OP` joining a `GROUP` ([Group members may arrive by
  splice](group-members-by-splice.md)) is this rule's operator case and carries no
  group-specific visibility handling: the member set extends at splice finalize,
  and the position gate alone decides which body expressions observe it.

**Directions.**

- *Closure unit — decided.* The declaration window — a block's top-level statement
  split, the boundary `MODULE`'s type announcement and `GROUP`'s member scan
  already read. Not a per-statement watermark: a watermark taken at each
  declaration would break the intra-block forward references that pending claims
  support today.
- *Splice batching — decided.* One window per `EVAL`, scanned off the spliced
  AST's own statement split and sealed when the splice finalizes. This is what
  makes a splice's interdependent declarations simultaneous, so no ordering
  question arises inside a splice.
- *Cross-window recursion — decided.* Refused. A splice cannot join the enclosing
  window, mirroring the type-side rule that a declaration computed at run time is
  not announced
  ([design/typing/user-types.md](../../design/typing/user-types.md)). Keeping the
  window parse-static is the property the whole rule rests on.
- *Gate representation — open.* (a) The body records its binder's cutoff index and
  the bucket walk filters candidates by it, reusing the type channel's existing
  `idx < cutoff` gate for a second channel; (b) each window materializes its own
  bucket view that the body's scope chain reaches. Recommended: (a) — one gate
  serving both channels is the point, and (b) adds a per-window structure to a
  scope chain that already carries the information.
- *Diagnostic siting for a split cycle — open.* Whether the refusal raises at the
  splice, which can see that a declaration it installs is referenced by an earlier
  window, or at the failed dispatch, where the miss actually surfaces. The first
  names the cause and the second names a real call site.

## Dependencies

Build-time dispatch resolution needs this rule to be able to commit a call site to
a target — an open bucket cannot be resolved while a later splice might still
contribute a more specific overload — so
[Two-phase execution](../editor_tooling/two-phase-execution.md) has a soft
interest here, ahead of any graded edge.

**Requires:**

- [EVAL splices in place](eval-splices-in-place.md) — the rule governs what a
  splice's declarations are visible to, so splicing must first be able to declare.

**Unblocks:** none tracked yet.
