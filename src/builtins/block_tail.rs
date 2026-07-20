//! The one "run a block, return the tail" constructor. EVAL, MATCH / TRY arms, and USING all mean
//! the same thing — run a body and yield its last statement as this slot's own structural terminal —
//! and each is a pure configuration of [`block_tail`]: a frame policy, a block scope, an optional
//! seed, and how the body maps to the tail. It is the sole site that builds an
//! [`Action::Tail`](crate::machine::Action::Tail); no builtin constructs one
//! elsewhere, so every "block, return the tail" terminal is one structural shape.

use std::rc::Rc;

use crate::machine::model::KExpression;
use crate::machine::model::TypeRegistry;
use crate::machine::Scope;
use crate::machine::{split_body_statements, ReturnContract};
use crate::machine::{Action, BlockEntry, FramePlacement, TailContract};

/// How the body maps onto the tail.
pub(crate) enum BlockBody<'a> {
    /// Tail-replace the whole expression, no split: splitting a single quoted expression would run a
    /// parenthesized group as a block.
    Single(KExpression<'a>),
    /// Split into leading statements + a tail; the leading statements run as owned deps before the tail.
    Block(KExpression<'a>),
}

/// The block scope the tail runs in — what `block_entry` names and where a `seed` binds.
pub(crate) enum BlockScope<'a> {
    /// No lexical block push; the tail runs in the frame's own scope with the chain unchanged.
    None,
    /// The `FreshChild` frame's own child scope is the block. The frame itself becomes `block_entry`,
    /// and a `seed` binds into its scope through
    /// [`CallFrame::with_scope`](crate::machine::CallFrame::with_scope).
    FrameOwn,
    /// A caller-allocated overlay scope in a cart-ancestor region. Its `id` becomes `block_entry`, and
    /// a `seed` binds into it directly.
    Overlay(&'a Scope<'a>),
}

/// A step run against the block scope before the tail dispatches. `for<'b>` so it binds whether the
/// block scope arrives as a short `with_scope` borrow (`FrameOwn`) or the `'a` overlay (`Overlay`).
/// The run's type registry arrives as a parameter rather than a capture: [`block_tail`] runs the
/// seed before it returns, so the seed borrows the registry for that call instead of owning a share.
pub(crate) type BlockSeed<'a> = Box<dyn for<'b> FnOnce(&Scope<'b>, &TypeRegistry) + 'a>;

/// Run a block and yield its last statement as the tail — the shared constructor.
pub(crate) fn block_tail<'a>(
    frame_placement: FramePlacement<'a>,
    block: BlockScope<'a>,
    seed: Option<BlockSeed<'a>>,
    body: BlockBody<'a>,
    contract: Option<ReturnContract<'a>>,
    types: &TypeRegistry,
) -> Action<'a> {
    let block_entry = match block {
        BlockScope::None => {
            debug_assert!(seed.is_none(), "a blockless tail takes no seed");
            BlockEntry::None
        }
        BlockScope::FrameOwn => {
            let FramePlacement::FreshChild { frame } = &frame_placement else {
                unreachable!("a FrameOwn block is the FreshChild frame's own scope");
            };
            if let Some(seed) = seed {
                frame.with_scope(|child| seed(child, types));
            }
            BlockEntry::FrameScope(Rc::clone(frame))
        }
        BlockScope::Overlay(overlay) => {
            if let Some(seed) = seed {
                seed(overlay, types);
            }
            BlockEntry::Overlay(overlay)
        }
    };
    let (leading, tail) = match body {
        BlockBody::Single(expr) => (Vec::new(), expr),
        BlockBody::Block(body) => {
            let mut statements = split_body_statements(body);
            let tail = statements
                .pop()
                .expect("split_body_statements always yields at least one");
            (statements, tail)
        }
    };
    Action::Tail {
        leading,
        tail,
        contract: TailContract::Eager(contract),
        frame_placement,
        block_entry,
    }
}
