//! The slot-step bracket restores the ambient values on unwind, not just on return.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;

use crate::builtins::default_scope;
use crate::machine::core::run_root_storage;
use crate::machine::execute::nodes::{NodePayload, NodeScope};
use crate::machine::execute::KoanRuntime;
use crate::machine::LexicalFrame;

#[test]
fn slot_step_bracket_restores_ambient_on_unwind() {
    let region = run_root_storage();
    let root = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    runtime.ensure_run_frame(root);
    let frame = runtime.run_frame_ref().expect("just established").clone();
    let payload = NodePayload {
        scope: NodeScope::Yoked,
        chain: LexicalFrame::detached(),
    };

    let result = catch_unwind(AssertUnwindSafe(|| {
        runtime.with_slot_step(frame, payload, true, |_rt| -> () {
            panic!("step body unwinds");
        })
    }));

    assert!(result.is_err());
    assert!(runtime.current_frame().is_none());
    assert!(!runtime.has_active_payload());
    assert!(!runtime.ambient.in_contract_chain());
}

#[test]
fn slot_step_bracket_restores_ambient_on_normal_return() {
    let region = run_root_storage();
    let root = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    runtime.ensure_run_frame(root);
    let frame = runtime.run_frame_ref().expect("just established").clone();
    let payload = NodePayload {
        scope: NodeScope::Yoked,
        chain: LexicalFrame::detached(),
    };

    let (_, post) = runtime.with_slot_step(frame.clone(), payload, true, |_rt| {});

    assert!(runtime.current_frame().is_none());
    assert!(!runtime.has_active_payload());
    assert!(!runtime.ambient.in_contract_chain());
    assert!(Rc::ptr_eq(&post.prev_frame, &frame));
}
