# RETURN from anywhere

Add an explicit `(RETURN <expr>)` form that ends the enclosing FN's body
with `<expr>`'s value, and tail-call-optimizes when `<expr>` is a
function call — from any position in the body, not just the last
statement.

**Problem.** Without `RETURN`, the body's terminal is its last
statement; tail-call optimization only fires when the recursive call
*is* that last statement. Tail-recursive patterns that branch early
(`IF base THEN 0 ELSE (recurse ...)`) work by nesting the recursive
call inside the last statement (typically a MATCH arm), which
composes — but at the cost of restructuring otherwise straight-line
code. There is no way to write "early-exit base case, recursive call
later" as flat statements and still get TCO.

**Impact.**

- *Early-exit base cases without restructuring.* `(IF (eq x 0) THEN
  (RETURN 0))` followed by `(FOO (sub x 1))` as the last statement
  reads top-to-bottom; today the user must invert the body into a
  MATCH or `IF-THEN-ELSE` wrapping the whole body.
- *TCO decoupled from "last statement".* The block-as-first-class
  concept ties TCO to the last statement (see `scratch/plan-block-as-
  first-class.md` D2). `RETURN` lets the tail-call live anywhere
  without giving up constant scheduler memory.
- *Explicit return-shape becomes part of the surface.* Aligns FN with
  imperative-language convention; pairs naturally with the
  `branch-arm-return-type` item (a MATCH arm could `RETURN` directly,
  bypassing the arm-value contract).

**Directions.**

- *Escape mechanism — open.* Three shapes:
  - *Error-as-control-flow:* `KErrorKind::EarlyReturn { value, tail_expr
    }`. RETURN's sub-slot errors with it; the FN's body Combine
    intercepts the variant in its error-propagation path and decodes
    into `Value` or `Tail`. **Trade-off:** if the FN slot has already
    `DeferTo`'d the Combine, the Combine slot tail-replaces — not the
    FN slot — so the frame reuse is one level off and TCO benefit is
    diluted.
  - *FN-slot-as-block-driver:* the FN slot runs body statements one at
    a time and replaces itself with the next (instead of `DeferTo`'ing
    a Combine over all). RETURN replaces the FN slot with the return
    expr directly, reusing its frame. **Trade-off:** rework of how FN
    bodies execute — touches `kfunction/invoke.rs`, the
    `BodyResult`/`NodeStep` shapes, and the scheduler's notion of
    "what work the FN slot owns". Cleanest semantics, biggest blast
    radius.
  - *Continuation handle:* RETURN takes an opaque `FnSlot` handle
    plumbed through the scheduler and tail-replaces it directly. **Trade-off:**
    new first-class object on the trait surface; cleanest if
    continuations become a recurring pattern.
- *Lexical reach — decided.* `RETURN` returns from the *innermost
  enclosing FN*, not from a MATCH / TRY arm or a nested block. A
  MATCH arm that wants to short-circuit the FN does so via `RETURN`;
  to short-circuit just the arm, the arm's value already serves.
- *Implicit-return interaction — open.* When the body ends without a
  `RETURN`, the last statement's value is still the terminal. `RETURN`
  is opt-in; existing code stays valid.
- *Tail detection — open.* RETURN of a function-call shape gets TCO;
  RETURN of a value or non-call expression returns directly. The
  syntactic check fires at RETURN's dispatch time on its inner
  expression.

## Dependencies

**Requires:** none.

**Unblocks:** none tracked yet.

FN / FUNCTOR / MATCH-arm / TRY-arm bodies now split into N statements
at invoke time and tail-replace into the last one (see
`design/execution-model.md` § Block submission). RETURN slots into
that model as either the error-as-control-flow shape (the Combine over
the body's N siblings catches a sentinel `EarlyReturn` error and
decodes it into a Value or a Tail) or the FN-slot-as-block-driver
shape (the FN slot owns statement iteration directly, so RETURN can
replace it in place from any position).
