//! Integration coverage for the dispatch-time wrap-slot eager resolve. Pins the four
//! shapes called out in the eager-wrap-resolve plan:
//!
//! - Bare leaf Type-token (`MAKESET int_ord`) — wrap-slot value resolves directly via
//!   `resolve_type_leaf_carrier`; the picked function binds without a sub-Dispatch detour.
//! - Forward Identifier reference in a wrap-slot — the eager pass parks on the
//!   producer's placeholder and re-dispatches on wake.
//! - Chained Type access (`:(LIST OF mo.Ty)`) — `Deferred` arm, not `wrap_indices`. Pinned
//!   here so the eager-path collapse doesn't accidentally route Deferred through the
//!   wrap-resolve loop.
//! - Parens-Expression wrap-slot (`MAKESET int_ord :| Ordered`) — the wrap-slot part
//!   is an `Expression`, not a bare token. The eager wrap-resolve no-ops; the lazy arm
//!   still sub-Dispatches the Expression.

use std::cell::RefCell;
use std::rc::Rc;

use koan::machine::interpret_with_writer;

/// Mirror the helper in `forward_reference_resolves.rs`: capture PRINT output into a
/// shared `Rc<RefCell<Vec<u8>>>` so tests can assert on what the program wrote.
fn run_capturing(source: &str) -> Result<String, koan::machine::KError> {
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    struct SharedBuf(Rc<RefCell<Vec<u8>>>);
    impl std::io::Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    interpret_with_writer(source, Box::new(SharedBuf(captured.clone())))?;
    let bytes = captured.borrow().clone();
    Ok(String::from_utf8(bytes).unwrap())
}

/// Bare leaf Type-token wrap-slot fast path. `MAKESET (int_ord :! Ordered)` carries
/// the ascription in parens so the inner sub-Dispatch is well-typed by the time the
/// MAKESET call dispatches; the fused splice/park/eager walk runs over an empty
/// `wrap_indices` (Future-bearing slot, not bare-name) and binds the picked function
/// directly with no subs to schedule. The returned module's `inner` member is `1`,
/// captured via PRINT.
#[test]
fn makeset_bare_type_token_resolves_eagerly() {
    let out = run_capturing(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)\n\
         FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET inner = 1))\n\
         LET my_set = (MAKESET (int_ord :! Ordered))\n\
         PRINT my_set.inner",
    )
    .expect("MAKESET on inline-ascribed int_ord should succeed");
    assert_eq!(out.trim(), "1", "expected printed `1`, got `{out}`");
}

/// Backward Identifier reference: a wrap-slot's bare-name part resolves to a still-
/// pending placeholder for an earlier-declared LET whose body hasn't terminalized
/// yet (the LET's RHS sub-Dispatch may park on a binder of its own). Eager resolve
/// parks the consumer on that placeholder; once the LET binder finalizes, the
/// re-dispatch reads the resolved value. Forward references no longer drive this
/// case under index-gated resolution — a later-sibling LET is invisible to the
/// consumer.
#[test]
fn wrap_slot_backward_identifier_parks_and_resumes() {
    let out = run_capturing(
        "FN (ECHO x :Number) -> Number = (x)\n\
         LET fwd = 42\n\
         LET result = (ECHO fwd)\n\
         PRINT result",
    )
    .expect("backward Identifier wrap-slot should resolve");
    assert_eq!(out.trim(), "42", "expected printed `42`, got `{out}`");
}

/// Chained-Type wrap-slot — `:(LIST OF mo.Ty)`-shape uses a parens-Expression, not a bare
/// token, so it hits the `Deferred` resolve path (no overload picks the bare shape; the
/// `mo.Ty` sub-Expression resolves first and the typed result re-dispatches via
/// `run_bind`). Pinned here to confirm the eager wrap-resolve collapse doesn't route
/// Deferred shapes through the wrap loop by accident.
#[test]
fn chained_type_access_uses_deferred_path() {
    // `mo.ty_value` is a chained-name expression — ATTR's `m.field` shape has a
    // sub-Expression on the right, so the wrap-slot eager-resolve isn't engaged.
    // Pinned so the eager-path collapse doesn't accidentally route Deferred shapes
    // through the wrap loop.
    let out = run_capturing(
        "MODULE mo = (LET ty_value = 99)\n\
         PRINT mo.ty_value",
    )
    .expect("mo.ty_value access should complete via the Deferred path");
    assert_eq!(out.trim(), "99", "expected printed `99`, got `{out}`");
}

/// Parens-Expression in a wrap-slot. The wrap-slot holds an `Expression`, not a bare
/// Type token, so eager wrap-resolve no-ops on that slot (classification only flags
/// bare-name parts as `wrap_indices`). The lazy-arm still schedules the inner
/// Expression as a sub-Dispatch. Result is `inner = 2` to distinguish from the
/// makeset_bare_type_token test above.
#[test]
fn wrap_slot_parens_expression_still_sub_dispatches() {
    let out = run_capturing(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)\n\
         FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET inner = 2))\n\
         LET my_set = (MAKESET (int_ord :| Ordered))\n\
         PRINT my_set.inner",
    )
    .expect("MAKESET with parens-Expression wrap-slot should sub-Dispatch");
    assert_eq!(out.trim(), "2", "expected printed `2`, got `{out}`");
}

/// `Slot::Park` literal arm — a forward Identifier inside a dict-value position. The
/// dict planner's `classify_aggregate_part` (with `wrap_identifiers = true`)
/// eager-resolves the bare name via `resolve_aggregate_bare_name`, hits a still-pending
/// placeholder, and records the producer in `park_producers`. On the LET binder's
/// terminalize, the dep-finish wakes and reads the value through the park-prefix
/// `Slot::Park(i)` lookup. Pins the wake-graph path that integration coverage
/// previously routed only through the dispatcher's wrap-slot path, not the literal
/// planner. (List literals use `wrap_identifiers = false` — bare identifiers there
/// stay `Static`, not eager-resolved — so the corresponding shape lives only in dict
/// keys/values.)
#[test]
fn dict_literal_backward_identifier_value_resolves_through_real_wake() {
    let out = run_capturing(
        "LET fwd = 99\n\
         LET m = {\"a\": fwd}\n\
         PRINT m",
    )
    .expect("backward Identifier in dict literal value should resolve");
    // Dict serialization wraps keys+values in `{}`.
    assert!(
        out.trim().contains("99"),
        "expected output to contain `99`, got `{out}`"
    );
    assert!(
        out.trim().contains("\"a\""),
        "expected output to contain `\"a\"`, got `{out}`"
    );
}

/// Lazy-candidate eager filter, exercised end-to-end through the fused walk.
/// `USING (some_module_expr) SCOPE (body)` makes USING a lazy candidate (its
/// `body:KExpression` slot binds the trailing parens-Expression), and the `m:Module`
/// slot's parens-Expression hits the `eager_indices` filter. The fused walk in
/// `run_dispatch` reads `resolved.slots.eager_indices` to gate the eager-sub
/// schedule: the m-slot sub-Dispatches, the SCOPE-body slot rides through
/// unscheduled, and the post-bind dep-finish re-dispatches with the resolved
/// `Future(KModule)` spliced into the m-slot.
#[test]
fn using_lazy_arm_with_filter_routes_module_expr_through_filter() {
    let out = run_capturing(
        "MODULE provider = (LET answer = 41)\n\
         LET ident_mod = provider\n\
         LET picked = (USING (ident_mod) SCOPE (answer))\n\
         PRINT picked",
    )
    .expect("USING with parens-Expression module slot should sub-Dispatch via filter");
    assert_eq!(out.trim(), "41", "expected printed `41`, got `{out}`");
}
