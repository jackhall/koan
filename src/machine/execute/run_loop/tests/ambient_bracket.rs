//! The slot-step bracket restores the ambient values on unwind, not just on return.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;

use crate::builtins::test_support::TestRun;
use crate::machine::core::run_root_storage;
use crate::machine::core::ReturnContract;
use crate::machine::execute::nodes::{NodePayload, NodeScope};
use crate::machine::execute::obligation::ReturnObligation;
use crate::machine::model::KType;
use crate::machine::LexicalFrame;

/// A trivial declared-return obligation the bracket tests deposit to stand in for the old
/// `in_contract_chain` bool: any obligation makes `in_contract_chain()` read `true` inside the step.
fn sample_obligation<'a>(scope: &'a crate::machine::Scope<'a>) -> ReturnObligation {
    let ret = scope.brand().alloc_ktype(KType::Number);
    ReturnObligation::seal(ReturnContract::Arm {
        ret,
        kind: "return type",
        scope,
    })
}

#[test]
fn slot_step_bracket_restores_ambient_on_unwind() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let root = test_run.scope;
    let obligation = sample_obligation(root);
    let runtime = &mut test_run.runtime;
    let frame = runtime.run_frame_ref().expect("seeded run frame").clone();
    let payload = NodePayload {
        scope: NodeScope::Yoked,
        chain: LexicalFrame::detached(),
    };

    let result = catch_unwind(AssertUnwindSafe(|| {
        runtime.with_slot_step(frame, payload, |rt| -> () {
            rt.ambient.deposit_obligation(obligation);
            assert!(
                rt.ambient.in_contract_chain(),
                "the deposited obligation makes the step a contract-chain step"
            );
            panic!("step body unwinds");
        })
    }));

    assert!(result.is_err());
    assert!(runtime.current_frame().is_none());
    assert!(!runtime.has_active_payload());
    assert!(
        !runtime.ambient.in_contract_chain(),
        "the obligation slot restores to empty through the unwind backstop"
    );
}

#[test]
fn slot_step_bracket_restores_ambient_on_normal_return() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let root = test_run.scope;
    let obligation = sample_obligation(root);
    let runtime = &mut test_run.runtime;
    let frame = runtime.run_frame_ref().expect("seeded run frame").clone();
    let payload = NodePayload {
        scope: NodeScope::Yoked,
        chain: LexicalFrame::detached(),
    };

    let (_, post) = runtime.with_slot_step(frame.clone(), payload, |rt| {
        rt.ambient.deposit_obligation(obligation);
    });

    assert!(runtime.current_frame().is_none());
    assert!(!runtime.has_active_payload());
    assert!(
        !runtime.ambient.in_contract_chain(),
        "the obligation slot restores to empty on the normal exit path"
    );
    assert!(
        post.obligation.is_some(),
        "the step's deposited obligation surfaces back out through PostStep"
    );
    assert!(Rc::ptr_eq(&post.prev_frame, &frame));
}
