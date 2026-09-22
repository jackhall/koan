//! Koan's step bundle: what a cell is born holding, what it parks with, and what it keeps in its
//! scratch habitat across a park.
//!
//! A **birth** crosses — a spawn or a tail hands it from one cell to another at the verdict's price
//! — so its family is covariant. The **parked state** never crosses: a park stores it in the cell's
//! own continuation and the wake hands it back at the same cell's brand, which is why the body
//! runner's state may hold its activation, which binds and so is invariant. See
//! [README.md § The bundle](README.md#the-bundle).

use crate::knot::{KActivationView, KValue, KnottedFamily};
use crate::memory::{CrossedOperand, DropFree, Writer, covariant, reattachable};
use crate::scheduler::StepBundle;
use crate::scope::Site;
use crate::values::copy_severed;

use super::body::Runner;
use super::record::{Evaluated, Program};

/// The bundle a koan program's steps run over.
pub struct KBundle;

/// What a cell is born holding.
#[derive(Clone, Copy)]
pub enum KBirth<'graph, 'cell> {
    /// The top level's root work.
    Program { program: &'graph Program<'graph> },
    /// A call: the frame's first step lays its activation down and binds the parameters from
    /// `arguments`, a record of them by name.
    Call {
        program: &'graph Program<'graph>,
        callee: KValue<'graph, 'cell>,
        arguments: KValue<'graph, 'cell>,
    },
    /// An evaluation: what it evaluates, and the view of the activation it reads names through.
    Evaluate {
        program: &'graph Program<'graph>,
        node: Evaluated<'graph>,
        view: KActivationView<'graph, 'cell>,
    },
    /// What the top level leaves at rest when it ends: its activation's view, for a later root
    /// work to read a top-level binding through.
    Inspect {
        program: &'graph Program<'graph>,
        view: KActivationView<'graph, 'cell>,
    },
}

/// What a cell holds at a step and parks with.
#[derive(Clone, Copy)]
pub enum KState<'graph, 'cell> {
    /// A cell's first step: what it was born with.
    Born(KBirth<'graph, 'cell>),
    /// The body runner, between units.
    Runner(Runner<'graph, 'cell>),
    /// An evaluator's resumption: what it was born with, and how far it got. Dispatch reshapes this
    /// arm; the tests' miniature evaluator needs nothing more.
    Evaluating {
        birth: KBirth<'graph, 'cell>,
        stage: u32,
    },
}

/// The family of [`KBirth`].
pub struct KBirthFamily;
reattachable!(KBirthFamily => KBirth<'graph, 'cell>);
// A birth crosses between cells, so its family carries the covariance witness — which is the check
// that an activation's view is covariant in its brand.
covariant!(KBirthFamily);
impl DropFree for KBirthFamily {}

/// The family of [`KState`].
pub struct KStateFamily;
reattachable!(KStateFamily => KState<'graph, 'cell>);

/// The family of what a parked cell keeps in its scratch habitat: the sites a refused tie named,
/// one per evaluation the runner asked for, in the order asked.
pub struct KScratchFamily;
reattachable!(both KScratchFamily => &'scratch [Site]);

impl<'graph> StepBundle<'graph> for KBundle {
    type Birth = KBirthFamily;
    type State = KStateFamily;
    type Scratch = KScratchFamily;

    /// A view is weighed at no bound, so the verdict never copies one: it is a borrow of another
    /// cell's region, and an evaluation or an inspection is born under the cell it views, where
    /// pinning it costs nothing.
    fn weight<'cell>(birth: &KBirth<'graph, 'cell>) -> usize
    where
        'graph: 'cell,
    {
        match birth {
            KBirth::Program { .. } => size_of::<usize>(),
            KBirth::Call {
                callee, arguments, ..
            } => callee.weight().plus(arguments.weight()).bytes(),
            KBirth::Evaluate { .. } | KBirth::Inspect { .. } => usize::MAX,
        }
    }

    fn cross<'cell>(
        writer: Writer<'cell>,
        view: &CrossedOperand<'graph, 'cell, '_, KBirthFamily>,
    ) -> KBirth<'graph, 'cell>
    where
        'graph: 'cell,
    {
        match view {
            CrossedOperand::Pinned { view, .. } => *view,
            CrossedOperand::Copied { view: copied, .. } => match *copied {
                KBirth::Program { program } => KBirth::Program { program },
                KBirth::Call {
                    program,
                    callee,
                    arguments,
                } => KBirth::Call {
                    program,
                    callee: copy_severed::<_, KnottedFamily>(writer, view, &callee),
                    arguments: copy_severed::<_, KnottedFamily>(writer, view, &arguments),
                },
                // Only a forced copy reaches here with a view — a tail hop's successor, which is a
                // sibling of the cell the view names. No step hops holding a view.
                KBirth::Evaluate { .. } | KBirth::Inspect { .. } => {
                    unreachable!("a birth holding a view is born under the cell it views")
                }
            },
        }
    }

    fn born<'cell>(birth: KBirth<'graph, 'cell>) -> KState<'graph, 'cell>
    where
        'graph: 'cell,
    {
        KState::Born(birth)
    }
}
