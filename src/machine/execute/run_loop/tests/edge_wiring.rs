//! Install-and-inspect: what the harness does when a park's source edge is **already filled** — its
//! producer terminalized and the finalize walk delivered into the edge before the park was wired.
//! The install door rules on that at wiring time (a filled edge, not a parked one), so the
//! classification is a return value rather than something the slot rediscovers at its next poll.
//! What rides on the ruling and is pinned here: the producer's own error is what reaches the
//! consumer, labelled with the park's own trace frame, and the resume never runs. The second test
//! marks the boundary — a producer that errors *after* the park is a different path, and lands
//! somewhere else.

use std::cell::Cell;
use std::rc::Rc;

use crate::builtins::test_support::TestRun;
use crate::machine::core::{program_storage, run_root_storage};
use crate::machine::{KError, KErrorKind, TraceFrame};
use crate::scheduler::ResolvedDeps;

use super::super::super::dispatch::park_resume_labelled;
use super::super::super::nodes::NodeWork;
use super::super::super::outcome::{Outcome, ignore_results};

/// A park whose source edge is already filled with an **error** surfaces that producer's error,
/// labelled with the park's own trace frame, and never runs the resume. The install door hands back
/// a filled edge sharing the source's resident, the consumer reads its terminal through it, and what
/// propagates is the producer's error verbatim — no re-derived verdict standing in for it.
///
/// The ok source is listed **first**, so its park is already wired when the errored one rules the
/// whole park out.
#[test]
fn park_on_errored_producer_propagates_producer_error() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;

    // Two ordinary producers, run to completion first: their walks deliver into the watch edges
    // below, so by the time the consumer parks, both source edges are filled rather than parked.
    let edge_ok = test_run.dispatch_watched_in(scope, super::let_expr(&program, "ok", 5.0));
    let edge_err = test_run.dispatch_watched_in(
        scope,
        super::working_one(&program, "LET bad = (undefined_thing)"),
    );
    let runtime = &mut test_run.runtime;
    runtime.execute().unwrap();
    assert!(
        runtime.edge_result_error(edge_err).is_err(),
        "the errored producer terminalized before the park is wired",
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
        &[],
        scope,
    );
    let watch = runtime.install_edge_for_test(consumer, scope);
    runtime.execute().unwrap();

    assert!(
        !resumed.get(),
        "a park whose source names an errored producer never reaches its resume",
    );
    let error = runtime
        .edge_result_error(watch)
        .err()
        .cloned()
        .expect("the consumer inherits the errored producer's terminal");
    assert!(
        matches!(&error.kind, KErrorKind::UnboundName(n) if n == "undefined_thing"),
        "expected the producer's own error, got {error}",
    );
    assert!(
        error.frames.iter().any(|f| f.function == "<test-park>"),
        "expected the park's labelling frame, got frames: {:?}",
        error.frames.iter().map(|f| &f.function).collect::<Vec<_>>(),
    );
}

/// The boundary of the ruling above, pinned end to end: install-and-inspect covers a park whose
/// source edge is **already** filled with an errored terminal. A binder that fails *after* a sibling
/// parked on it retires its claim as it terminalizes, so the woken sibling re-decides against a scope
/// where the name was never introduced and surfaces `UnboundName` — the name-introduction failure,
/// not the value failure. Retiring a failed binder's claim is what keeps a *later* sibling from
/// parking on a name that will never be bound, and this is the price of that.
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
    let watch = test_run.runtime.install_edge_for_test(ids[2], scope);
    let _ = test_run.runtime.execute();
    let error = test_run
        .runtime
        .edge_result_error(watch)
        .err()
        .cloned()
        .expect("the sibling of a failed binder cannot resolve its name");
    assert!(
        matches!(&error.kind, KErrorKind::UnboundName(n) if n == "z"),
        "expected UnboundName('z') from the retired claim, got {error}",
    );
}
