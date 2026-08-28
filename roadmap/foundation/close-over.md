# CLOSE OVER

**Problem.** A closure's captures carry reach: the closure value pins every
region its borrows lead to, and the pinning chains — a closure over a closure
retains the whole cactus of regions beneath it. Koan has no surface that
severs this: there is no way to build a value whose captures are copied
rather than pinned, so a value's retention footprint is dictated entirely by
where its captured values happen to live.

**Acceptance criteria.**

- With a data-only capture set, a closure defined in the block and escaping
  through the tail holds no reach into any region it was captured from, and a
  test observes that emptiness through the memory substrate (the producer
  frame's region frees while the closure remains callable).
- The data copy is transitive — everything a captured datum's borrows lead to
  is copied — since a shallow copy severs nothing.
- A pinned callable capture (explicit `_`-pattern or implicit close) names
  exactly the callable's home in the result's reach, and retention is
  transitive: the callable still runs correctly after every non-pinned
  producer frame has died.
- A capture list naming an in-flight binding parks the form and completes
  when the producer finishes.
- Capture-list tokens are identifiers or `_`-patterns only; dispatch
  registrations are never named by a bare keyword token.

**Directions.**

- *Surface — decided per [design/lazy-closures.md](../../design/lazy-closures.md).*
  `CLOSE OVER (captures ...) (block)`: the block runs over a dedicated
  region at the per-call tier with no `outer` link
  ([`RegionHost::fresh`](../../workgraph/src/witnessed/host.rs)); the tail
  value returns homed there; block-local bindings are invisible outside; the
  block scope's outer is the innermost eternal-homed scope of the enclosing
  chain, so builtins and top-level definitions stay visible and contribute
  no reach. `CLOSE OVER ()` is an empty capture.
- *Capture kinds — decided.* An identifier (value or type channel) resolves
  at the form's own step — parking on an in-flight placeholder through the
  standard resolve path — and relocates under the `Copy` verb: data rebuilt
  transitively, strings re-bumped, type handles copied by value. A
  signature-shaped pattern (`(HELPER _)`, `(MAP _ USING _)`) names one full
  untyped bucket key — never a bare lead keyword — and captures that
  registration pinned. `_` is a new first-class hole token (parser work;
  today it would lex as an identifier).
- *Implicit close of callables and modules — decided.* At block-scope build
  time, every dispatch registration and module binding in the per-call
  portion of the enclosing chain is copied into the block scope, pinned with
  its full reach. Retention is transitive from day 1 through the existing
  protocol — a pinned region retains its `FrameStorage.outer` chain and its
  binding entries' own pins. A build-time act, not call-time outward
  resolution: an escaped closure's body dispatches after its ancestors are
  dead.
- *Severing callable captures — deferred.* Callable/module leaves ride as
  pinned borrows under today's copy verb; the transitive callable copy is
  [lazy close](lazy-close.md).
- *EVAL — decided.* Permitted; a name resolving to neither the block scope,
  the captures, nor the eternal chain fails as unbound at the block
  boundary.

## Dependencies

The aspirational liveness-matrix design leans on this form as its proactive
consolidation lever
([liveness-matrix.md](../../workgraph/design/liveness-matrix.md)).

**Requires:** none — foundation.

**Unblocks:**

- [Frame recycling](../reduce_allocs/frame-recycling.md)
- [Lazy close](lazy-close.md)
- [Inferred CLOSE](inferred-close.md)
