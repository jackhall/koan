//! Dispatch: the [`Language`] koan's programs run under — the builtin table, the evaluator every
//! node in flight is handed to, and the load-time checks: the overlap check and static selection.
//!
//! A keyworded use's candidates are fixed where its shape is built: the builtin overloads at its
//! bucket key, then each registration visible to it, then — in a quote's code — the functions a
//! `USING` or an `EVAL` supplies for its key ([`CandidateList`](crate::scope::CandidateList)). A
//! call evaluates its slots, keeps the candidates whose expression shape admits the arguments'
//! types class by class — a quantified one's group solved from what the load knows of each
//! argument, and from its carried type where the load knows nothing — and ranks the survivors by
//! the type lattice's per-class verdicts;
//! a lone survivor runs, a builtin wins a tie, and anything else is a fault. A builtin
//! overload is a function value like any other, whose body is a native. Where the program loads,
//! each use's candidates are narrowed by its arguments' static types, and chosen where one is left
//! that admits them.
//!
//! - [`builtins`] lays the table down and runs the natives.
//! - [`evaluate`] is the evaluator: what a node is, and how its value is reached.
//! - [`select`] admits and ranks a call's candidates.
//! - [`rules`] gives each native's type rule: what its call's arguments need and what it returns.
//! - [`check`] refuses a registration that overlaps a builtin overload.
//! - [`walk`] is the load's one walk: every fact the load decides, unit by unit, into a store. It
//!   types every value expression and binder, and narrows and selects each keyworded use's
//!   candidates by those types.
//!
//! This file holds the vocabulary they share: what one evaluation is born over, and what one of
//! a call's operands came to. A submodule reaches it, and its siblings, through here.
//!
//! **Imports.** Outside `#[cfg(test)]` this module names `crate::builtin_image`,
//! `crate::elaborate`, `crate::knot`, `crate::memory`, `crate::parse`, `crate::program`,
//! `crate::scheduler`, `crate::scope`, `crate::symbols`, `crate::type_lattice` and
//! `crate::values`; `tests::boundary` reads the source to hold it there.
//!
//! See [dispatch/README.md](dispatch/README.md).

mod builtins;
mod caught;
mod check;
mod evaluate;
mod lookup;
mod rules;
mod select;
mod walk;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use builtins::overload_types;

use std::cell::Cell;

use crate::knot::{KBuiltins, KReading, KValue};
use crate::memory::{BumpAllocator, Writer};
use crate::parse::{ExpressionPart, KExpression};
use crate::program::{Catching, Delivered, Evaluated, Evaluator, Language, Owed, Program, Within};
use crate::scheduler::{Held, Use};
use crate::scope::{Code, Segment, ShapeError, Shapes, Store};
use crate::symbols::{BinderSymbol, SymbolInterner};
use crate::type_lattice::KType;
use crate::values::Chain;

/// Koan's language: what [`CellSubstrate::load`](crate::program::CellSubstrate::load) runs a
/// program under.
pub struct Koan;

impl Language for Koan {
    fn builtins<'graph>(
        writer: Writer<'graph, '_>,
        symbols: &'graph SymbolInterner,
        types: &Chain<'graph, 'graph, 'graph, '_>,
        scratch: BumpAllocator<'_>,
    ) -> &'graph KBuiltins<'graph, 'graph> {
        builtins::table(writer, symbols, types, scratch)
    }

    fn evaluator<'graph>() -> Evaluator<'graph> {
        evaluate::evaluate
    }

    fn facts<'graph: 'o, 'tier: 'o, 'o, S: Store<'o>>(
        shapes: &'graph Shapes<'graph>,
        builtins: &'graph KBuiltins<'graph, 'graph>,
        store: &S,
        types: &Chain<'tier, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        at: Segment,
    ) -> Result<(), ShapeError<'o>> {
        walk::walk(shapes, builtins, store, types, scratch, at)
    }
}

/// What one node in flight is evaluated over: the program, the view it reads names through beside
/// the step's door, what it owes a frame it finishes, what it runs within, the
/// node itself and the [`Use`] its value is placed by — beside the chain the step reads and
/// interns types through, built at the step's entry and never parked, and how far the node got:
/// its stage, the parts delivered to it so far, what a `TRY` or `CATCH` has copied of a fault, and
/// what was last delivered, taken where it is read. The evaluator writes the first three back to
/// the [`Node`](crate::program::Node) it was handed.
struct Evaluation<'step, 'graph, 'here, 'tier, 'reg, 'door, 'scratch> {
    program: &'graph Program<'graph>,
    code: Code<'step>,
    view: KReading<'step, 'graph, 'here>,
    owed: Option<Owed<'here>>,
    within: Within<'graph, 'here>,
    node: Evaluated,
    use_: Use,
    types: Chain<'tier, 'here, 'reg, 'door>,
    stage: Cell<u32>,
    received: Cell<&'scratch [Cell<Option<Held<'graph, 'here>>>]>,
    caught: Cell<Option<Catching<'graph, 'here>>>,
    delivered: Cell<Option<Delivered<'graph, 'here>>>,
}

/// What one of a call's operands came to: a value, beside the kept part it is where it has one, or
/// — an `ATTR` label written bare — the name it is written as.
#[derive(Clone, Copy)]
enum Operand<'graph, 'here> {
    Value(Held<'graph, 'here>),
    Label(BinderSymbol),
}

impl<'graph, 'here> Operand<'graph, 'here> {
    /// The type a candidate's slot admits the operand by: a value's carried type, or the code kind
    /// of the name a bare label is.
    fn ktype(&self) -> KType {
        match self {
            Operand::Value(held) => held.value.concrete_ktype(),
            Operand::Label(BinderSymbol::Type(_)) => KType::TYPE_NAME_TOKEN,
            Operand::Label(_) => KType::IDENTIFIER,
        }
    }

    fn value(&self) -> Option<KValue<'graph, 'here>> {
        match self {
            Operand::Value(held) => Some(held.value),
            Operand::Label(_) => None,
        }
    }
}

/// The name one-name code is — its lone part a value or a type name — as a quote's `node` holds it.
fn one_name(node: &KExpression<'_>) -> Option<BinderSymbol> {
    match node.parts {
        [only] => match only.value {
            ExpressionPart::Identifier(name) => Some(BinderSymbol::Value(name)),
            ExpressionPart::Type(name) => Some(BinderSymbol::Type(name)),
            _ => None,
        },
        _ => None,
    }
}
