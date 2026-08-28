//! "Run a block, return the tail" as one constructor. EVAL, MATCH / TRY arms, and USING all mean
//! the same thing — run a body and yield its last statement as this slot's own structural terminal —
//! and each is a pure configuration of [`block_tail`]: a frame policy, a block scope, an optional
//! seed, and how the body maps to the tail, so the shape of the
//! [`Action::Tail`](crate::machine::Action::Tail) they produce is settled in one place.

use std::rc::Rc;

use crate::machine::CallFrame;
use crate::machine::ReturnContract;
use crate::machine::Scope;
use crate::machine::core::RegionBrand;
use crate::machine::core::bindings::WriteGate;
use crate::machine::model::RunRegistries;
use crate::machine::model::{ExpressionPart, KExpression, WorkingExpression};
use crate::machine::{Action, BlockEntry, FramePlacement, TailContract};

/// How the body maps onto the tail.
pub(crate) enum BlockBody<'a> {
    /// Tail-replace the whole expression, no split: splitting a single quoted expression would run a
    /// parenthesized group as a block.
    Single(KExpression<'a>),
    /// Split into leading statements + a tail; the leading statements run as deps before the tail.
    Block(KExpression<'a>),
}

/// The block scope the tail runs in — what `block_entry` names and where a `seed` binds.
pub(crate) enum BlockScope<'a> {
    /// No lexical block push; the tail runs in the frame's own scope with the chain unchanged.
    None,
    /// A caller-allocated overlay scope in a cart-ancestor region. Its `id` becomes `block_entry`, and
    /// a `seed` binds into it directly.
    Overlay(&'a Scope<'a>),
    /// A **freshly minted per-call frame's own scope** is the block, and the frame is installed by
    /// the paired [`FramePlacement::FreshChild`](crate::machine::FramePlacement). The frame's child
    /// scope lives in the fresh region, which nothing but the frame `Rc` names yet, so a `seed`
    /// binds into it through the construction door — reached under
    /// [`CallFrame::with_scope`](crate::machine::CallFrame::with_scope), whose `for<'b>` brand is
    /// what confines the seeded values to the block's own region. `CLOSE OVER`'s block.
    FrameScope(Rc<CallFrame>),
}

/// The seed type a caller that passes none names, so `None` has a type to be `None` of. The binder
/// is written out rather than elided: elision would relate the reference and the scope's own
/// lifetime independently, which is not the rank-2 shape the seed bound quantifies over. Nothing is
/// ever called through it.
pub(crate) type NoSeed = for<'b> fn(&'b Scope<'b>, &RunRegistries, &mut WriteGate);

/// Bind a seed closure literal to the rank-2 signature [`block_tail`] quantifies over. `block_tail`
/// takes the seed as an `Option<S>`, which puts the bound a layer away from the closure's own type:
/// inference never sees it there and settles on independent lifetimes for the reference and the
/// scope it points at, which is not the shape the quantifier admits. Routing the literal through
/// this identity door states the signature at the point it is written, so its parameters can stay
/// unannotated.
pub(crate) fn seed<S>(f: S) -> S
where
    S: for<'b> FnOnce(&'b Scope<'b>, &RunRegistries, &mut WriteGate),
{
    f
}

/// Run a block and yield its last statement as the tail — the shared constructor. `brand` is the
/// region the working copies of the body's statements are frozen into: the body arrives as raw AST
/// and crosses to the scheduler here, at the point the tail is declared.
///
/// `seed` is a step run against the block scope before the tail dispatches, taken by value as
/// `impl FnOnce` so it stays a stack closure. The block scope reaches it as the caller's own `'a`
/// overlay. The run's registries arrive as a parameter rather than a capture: the seed runs before
/// this returns, so it borrows them for that call instead of owning a share.
///
/// The [`WriteGate`] arrives the same way. A seed binds into a block scope that has not dispatched
/// a statement yet — the construction door — but the seed itself is written builtin-side, where no
/// gate can be minted. `block_tail` mints one for the duration of the seed call and hands it in,
/// so the capability is the caller's to give, never the builtin's to take.
///
/// The seed's scope parameter is universally quantified: a [`BlockScope::FrameScope`] block is
/// reached only inside its frame's `with_scope` open, at a brand that outlives nothing, so a seed
/// cannot smuggle a value out of the block region it writes into. An overlay seed satisfies the
/// same bound — its scope is already at the caller's `'a`, which is one instantiation of the
/// quantifier.
pub(crate) fn block_tail<'a, S>(
    brand: RegionBrand<'a>,
    frame_placement: FramePlacement,
    block: BlockScope<'a>,
    seed: Option<S>,
    body: BlockBody<'a>,
    contract: Option<ReturnContract<'a>>,
    registries: &RunRegistries,
) -> Action<'a>
where
    S: for<'b> FnOnce(&'b Scope<'b>, &RunRegistries, &mut WriteGate),
{
    let block_entry = match block {
        BlockScope::None => {
            debug_assert!(seed.is_none(), "a blockless tail takes no seed");
            BlockEntry::None
        }
        BlockScope::Overlay(overlay) => {
            if let Some(seed) = seed {
                // The overlay is freshly allocated and has run nothing, so the seed writes through
                // the construction door.
                seed(overlay, registries, &mut WriteGate::for_unpublished_scope());
            }
            BlockEntry::Overlay(overlay)
        }
        BlockScope::FrameScope(frame) => {
            if let Some(seed) = seed {
                // The frame was minted by this call's own builtin and no node has reached its child
                // scope, so the same "unpublished scope" premise holds — structurally, not as a
                // claim the caller makes.
                frame.with_scope(|scope| {
                    seed(scope, registries, &mut WriteGate::for_unpublished_scope())
                });
            }
            BlockEntry::FrameScope(frame)
        }
    };
    // A body that is not a statement block splits to itself, so both no-split shapes take the same
    // lowering: an empty leading run — which borrows nothing and so allocates nothing — and the whole
    // expression as the tail. Only a real statement block reaches the split.
    let (leading, tail) = match body {
        BlockBody::Single(expr) => (&[][..], WorkingExpression::from_ast(brand, expr)),
        BlockBody::Block(body) if !body.is_statement_block() => {
            (&[][..], WorkingExpression::from_ast(brand, body))
        }
        BlockBody::Block(body) => {
            // The statements are frozen straight into `brand`'s region: a statement block's parts are
            // all expressions, so the run's length is the parts run's and the copies land in the
            // region the working expressions themselves name.
            let statements: &'a [WorkingExpression<'a>] =
                brand
                    .allocator()
                    .slice_from_iter(body.parts.iter().map(|part| {
                        let ExpressionPart::Expression(statement) = part.value else {
                            unreachable!("a statement block's parts are all expressions");
                        };
                        WorkingExpression::from_ast(brand, *statement)
                    }));
            let (tail, leading) = statements
                .split_last()
                .expect("a statement block carries at least two statements");
            (leading, *tail)
        }
    };
    Action::tail(
        leading,
        tail,
        TailContract::Eager(contract),
        frame_placement,
        block_entry,
    )
}
