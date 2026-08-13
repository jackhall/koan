//! Install-and-inspect: what the harness does when a park's source edge resolves to a producer that
//! has **already** terminalized. The install door rules on that at wiring time (a filled edge, not
//! a parked one), so the classification is a return value rather than something the slot rediscovers
//! at its next poll. Two things ride on the ruling and are pinned here: the producer's own error is
//! what reaches the consumer, and the rows the door wrote before the short-circuit still discharge.
//! The second test marks the boundary — a producer that errors *after* the park is a different
//! path, and lands somewhere else.

use std::cell::Cell;
use std::rc::Rc;

use crate::builtins::test_support::{TestRun, resident_carrier};
use crate::machine::core::{FrameCoverage, FrameStorageExt, program_storage, run_root_storage};
use crate::machine::model::{Carried, WorkingExpression};
use crate::machine::{KError, KErrorKind, TraceFrame};
use crate::scheduler::ResolvedDeps;

use super::super::super::dispatch::park_resume_labelled;
use super::super::super::nodes::NodeWork;
use super::super::super::outcome::{Outcome, ignore_results};

/// A park whose source edge names an already-errored producer surfaces **that producer's** error,
/// labelled with the park's own trace frame, and never runs the resume. The install door hands back
/// a filled edge, the harness reads the terminal through it, and what propagates is the producer's
/// error verbatim — no re-derived verdict standing in for it.
///
/// The ok park is installed **first**, so its row and its late-pull debt are already written when
/// the errored one short-circuits the whole park — the leak shape this also pins.
#[test]
fn park_on_errored_producer_propagates_producer_error_and_discharges_installed_rows() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;

    // Two synthetic terminals: one delivering a value, one errored. Both are dispatch slots drained
    // out of the queue before their results are written by hand, so `execute` never revisits them.
    let mk_dispatch = || {
        crate::machine::execute::dispatch::decide_tail(
            WorkingExpression::new(program.brand().region(), Vec::new()),
            None,
        )
    };
    let producer_ok = runtime.add(mk_dispatch(), scope);
    let producer_err = runtime.add(mk_dispatch(), scope);
    let store = runtime.scheduler_mut();
    store.clear_node(producer_ok);
    store.clear_node(producer_err);
    let _ = store.pop_next();
    let _ = store.pop_next();
    let value = region
        .brand()
        .alloc_scalar(crate::machine::model::Scalar::Number(5.0));
    store.set_result(
        producer_ok,
        Ok(Carried::Object(value)),
        resident_carrier(scope),
    );
    // A hand-written terminal carries no finalize-seeded hold; seed one so the consumer's install
    // has a retention count to bump and the discharge below has something to observe.
    store.seed_retention(producer_ok, Rc::clone(&region), FrameCoverage::empty(), 1);
    store.set_result(
        producer_err,
        Err(KError::new(KErrorKind::ShapeError(
            "producer synthetic".into(),
        ))),
        resident_carrier(scope),
    );

    let edge_ok = runtime.install_claim_edge_for_test(producer_ok, scope);
    let edge_err = runtime.install_claim_edge_for_test(producer_err, scope);
    assert_eq!(
        runtime.scheduler().retained_pulls(producer_ok),
        Some(1),
        "the claim edge alone owes no pull — only a consumer's install does",
    );

    let resumed: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let resumed_in_step = Rc::clone(&resumed);
    let frame = TraceFrame::bare("<test-park>", "park on two producers");
    let consumer = runtime.add(
        NodeWork::new(
            ResolvedDeps::new(),
            ignore_results(Box::new(move |_view, _id| {
                park_resume_labelled(
                    vec![edge_ok, edge_err],
                    None,
                    Some(frame),
                    Box::new(move |_view, _id| {
                        resumed_in_step.set(true);
                        Outcome::Done(Err(KError::new(KErrorKind::ShapeError(
                            "resume ran".into(),
                        ))))
                    }),
                )
            })),
            None,
        ),
        scope,
    );
    runtime.execute().unwrap();

    assert!(
        !resumed.get(),
        "a park whose source names an errored producer never reaches its resume",
    );
    let error = runtime
        .result_error(consumer)
        .err()
        .cloned()
        .expect("the consumer inherits the errored producer's terminal");
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(m) if m == "producer synthetic"),
        "expected the producer's own error, got {error}",
    );
    assert!(
        error.frames.iter().any(|f| f.function == "<test-park>"),
        "expected the park's labelling frame, got frames: {:?}",
        error.frames.iter().map(|f| &f.function).collect::<Vec<_>>(),
    );

    // The ok park's row was written before the errored one ruled the park out. The consumer owes
    // that late pull exactly as an un-short-circuited park would, and its death discharges it —
    // back to the seeded count, not one above it.
    assert_eq!(
        runtime.scheduler().retained_pulls(producer_ok),
        Some(2),
        "the installed park owes a late pull on the already-finalized producer",
    );
    runtime.free(consumer);
    assert_eq!(
        runtime.scheduler().retained_pulls(producer_ok),
        Some(1),
        "the short-circuited park discharges the row the door had already installed",
    );
}

/// The boundary of the ruling above, pinned end to end: install-and-inspect covers a park whose
/// source **already** names an errored terminal. A binder that fails *after* a sibling parked on it
/// retires its claim as it terminalizes, so the woken sibling re-decides against a scope where the
/// name was never introduced and surfaces `UnboundName` — the name-introduction failure, not the
/// value failure. Retiring a failed binder's claim is what keeps a *later* sibling from parking on
/// a name that will never be bound, and this is the price of that.
#[test]
fn a_binder_that_fails_after_its_sibling_parked_surfaces_unbound_name() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let ids = test_run.runtime.enter_block(
        scope.id,
        super::working_all(
            &program,
            "FN (BOOM) -> Any = (undefined_thing)\nLET z = (BOOM)\nLET y = (z)",
        ),
        scope,
    );
    let _ = test_run.runtime.execute();
    let error = test_run
        .runtime
        .result_error(ids[2])
        .err()
        .cloned()
        .expect("the sibling of a failed binder cannot resolve its name");
    assert!(
        matches!(&error.kind, KErrorKind::UnboundName(n) if n == "z"),
        "expected UnboundName('z') from the retired claim, got {error}",
    );
}
