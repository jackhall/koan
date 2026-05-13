# Lift Scope::pending into a PendingQueue façade

**Problem.** `Scope::pending` and the `PendingWrite` enum live as a
bare `RefCell<Vec<PendingWrite<'a>>>` field plus an inline tagged enum
on [`Scope`](../src/runtime/machine/core/scope.rs) (scope.rs:48-52, 69).

- *Duplicated try-direct/fall-back pattern.* The "try direct, fall
  back to queue" shape appears verbatim in `bind_value`
  (scope.rs:135-143) and `register_function` (scope.rs:151-168). Both
  match on `ApplyOutcome::Conflict` and push a `PendingWrite`
  variant by hand.
- *Silent `Err(_)` drop on drain.* `drain_pending` (scope.rs:285-316)
  discards errors from retry attempts. The drop is load-bearing —
  drain is invoked from `execute/scheduler/execute.rs` between
  dispatch nodes, where no caller frame is alive to surface a
  `KError` to — but the silence hides classes of programmer error
  with no diagnostic signal.
- *Variant tag is module-fuzzy.* `PendingWrite` is module-private but
  its variant constructors are written inline at the two write
  sites; adding a new write kind today requires threading a new
  variant through three code paths (constructor, drain match arm,
  validated-retry path).

**Impact.**

- Deferred writes route through a single `PendingQueue<'a>` API whose
  `defer_value` / `defer_function` mirror the `Bindings` write
  surface, and the try-then-defer pattern collapses to one match arm
  per write kind:
  ```rust
  match self.bindings.try_bind_value(&name, obj)? {
      ApplyOutcome::Applied => Ok(()),
      ApplyOutcome::Conflict => { self.pending.defer_value(name, obj); Ok(()) }
  }
  ```
- `PendingWrite` becomes module-private to `PendingQueue`. Adding a
  new write kind requires changes inside one type instead of
  threading a variant through three code paths.
- `drain` takes `&Bindings<'a>` and routes deferred retries through
  `try_apply` directly, so the validated-write-path invariant
  (single dual-map mirror, structural dedupe) extends to drained
  retries by construction.
- A `debug_assert!` on drain-time errors surfaces queue/dispatch
  interaction bugs immediately in debug builds. Production keeps the
  current `Err(_)`-drop behavior so dispatch nodes never see surfaced
  errors. With the `Bindings` façade in place, a deferred LET-binds-fn
  write can legitimately fail with `DuplicateOverload` (structural
  dedupe), so the assert closes a real visibility gap.

**Directions.**

- *Lift `pending` and `PendingWrite` into `PendingQueue<'a>` —
  decided.* Shape:
  ```rust
  pub struct PendingQueue<'a> {
      pending: RefCell<Vec<PendingWrite<'a>>>,
  }
  ```
  `PendingWrite` becomes private to the module.
- *Surface — decided.* `defer_value(name, &KObject)`,
  `defer_function(name, &KFunction, &KObject)`, `drain(&Bindings<'a>)`.
  `drain` is the sole consumer; taking `&Bindings` directly forces
  retries through the same validated path as direct writes.
- *Drain error posture — decided.* `debug_assert!(result.is_ok(),
  "...")` with a doc comment naming the invariant: "by drain time
  these are invariant violations." Production builds keep the
  current `Err(_)`-drop behavior so dispatch nodes never see
  surfaced errors.
- *`std::mem::take` on drain — decided.* Stays. `try_apply` may
  itself `defer` on contention during drain, so the queue must move
  out before retry; otherwise the borrow re-entry would deadlock.

## Dependencies

**Requires:** none. The `Bindings<'a>` façade shipped first as shared
substrate this work routes through — `drain`'s `&Bindings<'a>` parameter
and the collapsed "try-then-defer" call sites depend on
`Bindings::try_apply` existing as the shared validated write path.

**Unblocks:** none.
