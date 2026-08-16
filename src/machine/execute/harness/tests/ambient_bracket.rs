//! The slot-step bracket restores the ambient values on unwind, not just on return.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use crate::builtins::test_support::TestRun;
use crate::machine::LexicalFrame;
use crate::machine::core::run_root_storage;
use crate::machine::core::{ReturnContract, program_storage};
use crate::machine::execute::nodes::{NodePayload, NodeScope};
use crate::machine::execute::obligation::ReturnObligation;
use crate::machine::model::KType;

/// A trivial declared-return obligation the bracket tests deposit: any deposited obligation makes
/// `current_obligation_duplicate()` answer `Some` inside the step.
fn sample_obligation() -> ReturnObligation {
    ReturnObligation::seal(ReturnContract::Arm {
        ret: KType::NUMBER,
        kind: "return type",
    })
}

#[test]
fn slot_step_bracket_restores_ambient_on_unwind() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let obligation = sample_obligation();
    let host = &mut test_run.runtime.host;
    let frame = host
        .ambient
        .run_frame_ref()
        .expect("seeded run frame")
        .clone();
    let payload = NodePayload {
        scope: NodeScope::Yoked,
        chain: LexicalFrame::root(test_run.scope.id, 1),
    };

    let result = catch_unwind(AssertUnwindSafe(|| {
        host.with_slot_step(frame, payload, |host| -> () {
            host.ambient.deposit_obligation(obligation);
            assert!(
                host.ambient.current_obligation_duplicate().is_some(),
                "the deposited obligation makes the step a contract-chain step"
            );
            panic!("step body unwinds");
        })
    }));

    assert!(result.is_err());
    let host = &mut test_run.runtime.host;
    assert!(host.ambient.active_frame_ref().is_none());
    assert!(host.ambient.active_payload().is_none());
    assert!(
        host.ambient.current_obligation_duplicate().is_none(),
        "the obligation slot restores to empty through the unwind backstop"
    );
}

#[test]
fn slot_step_bracket_restores_ambient_on_normal_return() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let obligation = sample_obligation();
    let host = &mut test_run.runtime.host;
    let frame = host
        .ambient
        .run_frame_ref()
        .expect("seeded run frame")
        .clone();
    let payload = NodePayload {
        scope: NodeScope::Yoked,
        chain: LexicalFrame::root(test_run.scope.id, 1),
    };

    let step_end_frame = host.with_slot_step(frame.clone(), payload, |host| {
        host.ambient.deposit_obligation(obligation);
        // The apply tail reads the deposit and the step-end cart off the ambient context inside
        // the bracket — the reads the Done arm's obligation gate makes.
        assert!(
            host.ambient.current_obligation_duplicate().is_some(),
            "the step's deposited obligation is an ambient read within the bracket"
        );
        host.ambient
            .active_frame_ref()
            .expect("a step always runs against a cart")
            .clone()
    });

    assert!(host.ambient.active_frame_ref().is_none());
    assert!(host.ambient.active_payload().is_none());
    assert!(
        host.ambient.current_obligation_duplicate().is_none(),
        "the obligation slot restores to empty on the normal exit path"
    );
    assert!(Rc::ptr_eq(&step_end_frame, &frame));
}
