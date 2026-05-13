# Encapsulate Scope write paths in a Bindings façade

**Problem.** Three `RefCell`s on `Scope` —
[`data`, `functions`, `placeholders`](../src/runtime/machine/core/scope.rs)
— co-mutate in lockstep across `try_apply_value` (scope.rs:210-246),
`try_apply_function` (scope.rs:251-280), `apply_function_data_insert`,
and `install_placeholder`. No type enforces the co-mutation.

- *Dual-map mirror enforced by convention.* Every entry in `data` that
  wraps a `KFunction` must also live in
  `functions[signature.untyped_key()]`. Two write paths reach into
  both maps independently; the invariant is reasserted in each.
- *Latent LET-binds-FN bug.* `try_apply_value`'s `KFunction` arm
  (scope.rs:235) dedupes by `ptr::eq`; `try_register_function`
  (scope.rs:267-276) uses `signatures_exact_equal`. A user writing
  ```
  FN x:Number -> ...
  LET f = (FN x:Number -> ...)
  ```
  silently doubles the dispatch bucket — same key, two entries,
  dispatch ties.
- *One production bypass.*
  [`ascribe.rs:34-48`](../src/runtime/builtins/ascribe.rs) reaches into
  `data.borrow_mut().insert(...)` directly while iterating
  `src.data.borrow()` and `src.functions.borrow()`, mirroring the
  `functions` map by hand. The bypass exists because going through
  `bind_value` today would double-register each `KFunction` (once via
  the dual-map mirror, once via the explicit functions loop).
- *Test-setup bypasses.*
  [`value_lookup.rs:110,151`](../src/runtime/builtins/value_lookup.rs)
  inside `#[cfg(test)]` modules reach into the `RefCell`s directly to
  seed test fixtures.

**Impact.**

- Every write to scope bindings routes through a single `Bindings<'a>`
  API that enforces the dual-map mirror and structural dedupe in one
  shared helper. The two `try_apply_*` paths collapse into one.
- The LET-binds-FN dedupe gap closes: structurally identical
  signatures trip `DuplicateOverload` whether declared via `FN` or
  via `LET f = (FN ...)`. A new test pins the behavior beside
  `register_function_dedupes_exact_signature` in `scope.rs`.
- `ascribe` stops "knowing" the dual-map exists. The explicit
  `functions` loop in `ascribe.rs:41-48` collapses into a single
  `Bindings::try_bulk_install_from` call that replays `src.data` and
  lets the shared helper re-mirror into `functions` exactly once.
- The non-fn shortcut (skip `functions` borrow when the payload is
  not a `KFunction`) is preserved by construction inside the shared
  helper. `register_type`, LET-body, and param-binding flows that
  bind under a live outer `functions` borrow stay deadlock-free
  without relying on convention.
- Read access stays one method call away: `Bindings::data()` and
  `Bindings::functions()` return `Ref<'_, _>` guards so the ~12
  read-only call sites in builtins and `resolve_dispatch`'s
  outer-chain walk keep their existing borrow ordering.

**Directions.**

- *Lift `data`, `functions`, and `placeholders` into `Bindings<'a>` —
  decided.* Placeholders move with the other two because every
  placeholder mutation already happens alongside a `data` or
  `functions` mutation (clear-on-bind in `try_apply_value`,
  reject-on-rebind in `install_placeholder`).
- *Shared private helper `try_apply(name, obj, fn_part)` — decided.*
  Signature:
  ```rust
  fn try_apply(
      &self,
      name: &str,
      obj: &'a KObject<'a>,
      fn_part: Option<&'a KFunction<'a>>,
  ) -> Result<ApplyOutcome, KError>
  ```
  Borrows `functions` only when `fn_part.is_some()`, then `data`,
  then dual-map insert with unified dedupe (`ptr::eq` *or*
  `signatures_exact_equal`). Public `try_bind_value` /
  `try_register_function` delegate.
- *Read accessors `data()` and `functions()` returning `Ref<'_, _>` —
  decided.* `resolve_dispatch`'s explicit `drop(functions)`
  (scope.rs:464) becomes `drop(functions_guard)` before outer-chain
  recursion. No `into_outer`-style consumer needed.
- *`try_install_placeholder` infallible in the borrow-conflict
  dimension — decided.* Uses `borrow_mut`, not `try_borrow_mut`;
  `Result` is for semantic rejection only. Doc-comment the asymmetry
  with the `try_bind_*` methods.
- *Add `try_bulk_install_from(src: &Bindings<'a>)` — decided.*
  Snapshots `src.data` into a `Vec`, releases `src`'s `Ref` guard,
  replays each entry through `try_apply` on `self`. Ascribe migrates
  to this; the explicit functions loop is deleted because the shared
  helper re-mirrors into `functions` exactly once.
- *Test for LET-binds-FN `DuplicateOverload` — decided.* Beside
  `register_function_dedupes_exact_signature` in
  [`scope.rs`](../src/runtime/machine/core/scope.rs) tests. Asserts
  the LET path now trips `DuplicateOverload` for a structurally
  identical prior FN. Lands in the same PR as the split.
- *Migrate `value_lookup.rs:110,151` test setup to `try_bind_value` —
  decided.* No dedicated `#[cfg(test)] insert_for_test` escape hatch
  — the production surface is enough.

## Dependencies

**Requires:** none. The `Resolved`/`ClassifiedSlots` fold and the
runtime-machine unsafe-surface reduction shipped first as shared
substrate this façade routes through.

**Unblocks:** [pending-queue-facade](pending-queue-facade.md) — the
PendingQueue façade's `drain(&Bindings<'a>)` and the collapsed
"try-then-defer" call sites depend on `Bindings::try_apply` existing.
