//! Koan's step bundle: what a cell is born holding, what it parks with, and what it keeps in its
//! scratch habitat across a park.
//!
//! A **birth** crosses — a spawn or a tail hands it from one cell to another at the verdict's price
//! — so its family is covariant. The **parked state** never crosses: a park stores it in the cell's
//! own continuation and the wake hands it back at the same cell's brand, which is why the body
//! runner's state may hold its activation, which binds and so is invariant. See
//! [README.md § The bundle](README.md#the-bundle).

use crate::knot::{KActivationView, KValue, KnottedFamily};
use crate::memory::{CrossedOperand, DropFree, Header, Writer, collect, covariant, reattachable};
use crate::scheduler::{Born, StepBundle};
use crate::scope::{BodyShape, Site};
use crate::type_lattice::KType;
use crate::values::{Half, TypeHeader, Types, copy_severed};

use super::body::Runner;
use super::record::{CallKind, Contract, Evaluated, Program};

/// The bundle a koan program's steps run over.
pub struct KBundle;

/// What a cell is born holding.
#[derive(Clone, Copy)]
pub enum KBirth<'graph, 'cell> {
    /// The top level's root work.
    Program { program: &'graph Program<'graph> },
    /// A call: the frame's first step lays its activation down and binds the parameters from
    /// `arguments`, a record of them by name, checked as `kind` says. A call by name solves each
    /// parameter from its entry in `contributed`, one per parameter in symbol order where the load
    /// recorded any, and from its argument's carried type where that entry is `None` or there are
    /// none. `owed` is the contract of the evaluation that tailed into this frame: `None` when the
    /// frame was spawned.
    Call {
        program: &'graph Program<'graph>,
        callee: KValue<'graph, 'cell>,
        arguments: KValue<'graph, 'cell>,
        kind: CallKind,
        contributed: &'cell [Option<KType>],
        owed: Option<Contract>,
    },
    /// An `EVAL`: the frame's first step lays the code's activation down over the bindings it
    /// carries and the names `offered`, a record of them by name, supplies — and `contract`, the
    /// `EVAL`'s declared return, which the frame owes.
    Eval {
        program: &'graph Program<'graph>,
        code: KValue<'graph, 'cell>,
        offered: KValue<'graph, 'cell>,
        contract: Contract,
    },
    /// An evaluation: what it evaluates, and the view of the activation it reads names through.
    /// A frame's tail hands its last statement over with the frame's `contract`, which the
    /// evaluation then owes its value.
    Evaluate {
        program: &'graph Program<'graph>,
        node: Evaluated<'graph>,
        view: KActivationView<'graph, 'cell>,
        contract: Option<Contract>,
    },
    /// A block — a synthesized block part's shape, or a `USING … SCOPE` body — run beside the
    /// view of the activation it sits in, its last statement's value its own; a `USING` body's
    /// parameters bound to what `module` surfaces.
    Block {
        program: &'graph Program<'graph>,
        shape: &'graph BodyShape<'graph>,
        enclosing: KActivationView<'graph, 'cell>,
        module: Option<KValue<'graph, 'cell>>,
    },
    /// What the top level leaves at rest when it ends: its activation's view, for a later root
    /// work to read a top-level binding through.
    Inspect {
        program: &'graph Program<'graph>,
        view: KActivationView<'graph, 'cell>,
    },
}

impl<'graph, 'cell> KBirth<'graph, 'cell> {
    /// Every value the birth carries itself — not those an activation it views holds — for the
    /// crossing's habitat check.
    #[cfg(debug_assertions)]
    fn values(&self, out: &mut impl FnMut(&KValue<'graph, 'cell>)) {
        match self {
            KBirth::Call {
                callee, arguments, ..
            } => {
                out(callee);
                out(arguments);
            }
            KBirth::Eval { code, offered, .. } => {
                out(code);
                out(offered);
            }
            KBirth::Block {
                module: Some(module),
                ..
            } => out(module),
            _ => {}
        }
    }

    /// The program every birth carries.
    pub fn program(&self) -> &'graph Program<'graph> {
        match *self {
            KBirth::Program { program }
            | KBirth::Call { program, .. }
            | KBirth::Eval { program, .. }
            | KBirth::Evaluate { program, .. }
            | KBirth::Block { program, .. }
            | KBirth::Inspect { program, .. } => program,
        }
    }
}

/// What a cell holds at a step and parks with.
#[derive(Clone, Copy)]
pub enum KState<'graph, 'cell> {
    /// A cell's first step: what it was born with.
    Born(KBirth<'graph, 'cell>),
    /// The body runner, between units.
    Runner(Runner<'graph, 'cell>),
    /// An evaluator's resumption: what it was born with, and how far it got — which parts of the
    /// node it has asked for is read again off the node itself.
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
    /// cell's region. An evaluation, a block or an inspection is born under the cell it views,
    /// where pinning it costs nothing, or is a frame's `Shares` tail, which pins the frame into the
    /// ancestor its successor is a tenant of.
    fn weight<'cell>(birth: &KBirth<'graph, 'cell>) -> usize
    where
        'graph: 'cell,
    {
        match birth {
            KBirth::Program { .. } => size_of::<usize>(),
            KBirth::Call {
                callee,
                arguments,
                contributed,
                ..
            } => callee.weight().plus(arguments.weight()).bytes() + size_of_val(*contributed),
            KBirth::Eval { code, offered, .. } => code.weight().plus(offered.weight()).bytes(),
            KBirth::Evaluate { .. } | KBirth::Block { .. } | KBirth::Inspect { .. } => usize::MAX,
        }
    }

    /// A pinned birth embeds as it is, and its home's branch reaches the cell's: a spawned child's
    /// branch links it, and a hop's successor or a left birth links it too unless the branch
    /// already links one — a tenant of a linked host's — where it merges it. A copied one is rebuilt, each
    /// type its values carry adopted from the home's branch.
    fn cross<'cell>(
        writer: Writer<'cell>,
        header: Header<'graph, 'cell, TypeHeader>,
        view: &CrossedOperand<'graph, 'cell, '_, KBirthFamily, TypeHeader>,
        born: Born,
    ) -> KBirth<'graph, 'cell>
    where
        'graph: 'cell,
    {
        match view {
            CrossedOperand::Pinned { view, home, .. } => {
                let types = Types::new(view.program().root(), Half::of(writer, header));
                home.read(|header, _| match born {
                    Born::Child => types.link(header.into_value()),
                    Born::Hop | Born::Left => types.link_or_merge(header.into_value()),
                });
                #[cfg(debug_assertions)]
                view.values(&mut |value| {
                    crate::values::check_pinned_habitat::<KnottedFamily>(&types, value)
                });
                *view
            }
            CrossedOperand::Copied { view: copied, .. } => match *copied {
                KBirth::Program { program } => KBirth::Program { program },
                // The values of one birth cross through one copy, so they share each knot.
                KBirth::Call {
                    program,
                    callee,
                    arguments,
                    kind,
                    contributed,
                    owed,
                } => {
                    let types = Types::new(program.root(), Half::of(writer, header));
                    let [callee, arguments] = copy_severed::<_, KnottedFamily, 2>(
                        writer,
                        &types,
                        view,
                        [&callee, &arguments],
                    );
                    KBirth::Call {
                        program,
                        callee,
                        arguments,
                        kind,
                        contributed: collect(writer, contributed.iter().copied()),
                        owed,
                    }
                }
                KBirth::Eval {
                    program,
                    code,
                    offered,
                    contract,
                } => {
                    let types = Types::new(program.root(), Half::of(writer, header));
                    let [code, offered] = copy_severed::<_, KnottedFamily, 2>(
                        writer,
                        &types,
                        view,
                        [&code, &offered],
                    );
                    KBirth::Eval {
                        program,
                        code,
                        offered,
                        contract,
                    }
                }
                // Only a forced copy reaches here with a view: a `Fresh` hop's successor, a sibling
                // of the cell the view names. A view is born under the cell it views, or handed on
                // by a `Shares` hop, whose successor is a tenant of an ancestor and pins it.
                KBirth::Evaluate { .. } | KBirth::Block { .. } | KBirth::Inspect { .. } => {
                    unreachable!("a birth holding a view is never copied")
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
