//! Dispatch: the [`Language`] koan's programs run under — the builtin table, the step every
//! evaluation runs, and the load-time checks: the overlap check and static selection.
//!
//! A keyworded use's candidates are fixed where its shape is built: the builtin overloads at its
//! bucket key, then each registration visible to it, then — in a quote's code — the functions a
//! `USING` or an `EVAL` supplies for its key ([`CandidateList`](crate::scope::CandidateList)). A
//! call evaluates its slots, keeps the candidates whose expression shape admits the arguments'
//! carried types class by class, and ranks the survivors by the type lattice's per-class verdicts;
//! a lone survivor runs, a builtin wins a tie, and anything else is an error value. A builtin
//! overload is a function value like any other, whose body is a native. Where the program loads,
//! each use's candidates are narrowed by its arguments' static types, and chosen where one is left
//! that admits them.
//!
//! - [`builtins`] lays the table down and runs the natives.
//! - [`evaluate`] is the step: what a node is, and how its value is reached.
//! - [`select`] admits and ranks a call's candidates.
//! - [`rules`] gives each native's type rule: what its call's arguments need and what it returns.
//! - [`check`] refuses a registration that overlaps a builtin overload.
//! - [`statics`] types every value expression and binder where the program loads, and narrows and
//!   selects each keyworded use's candidates by those types.
//! - [`errors`] renders the messages of the error values dispatch raises.
//!
//! This file holds the vocabulary they share: what one evaluation is born over, and what one of
//! a call's operands came to. A submodule reaches it, and its siblings, through here.
//!
//! **Imports.** Outside `#[cfg(test)]` this module names `crate::elaborate`, `crate::knot`,
//! `crate::memory`, `crate::parse`, `crate::program`, `crate::scheduler`, `crate::scope`,
//! `crate::symbols`, `crate::type_lattice` and `crate::values`; `tests::boundary` reads the source
//! to hold it there.
//!
//! See [dispatch/README.md](dispatch/README.md).

mod builtins;
mod check;
mod errors;
mod evaluate;
mod rules;
mod select;
mod statics;

#[cfg(test)]
mod tests;

use crate::knot::{KActivationView, KBuiltins, KValue};
use crate::memory::{BumpAllocator, Writer};
use crate::parse::{ExpressionPart, KExpression};
use crate::program::{Contract, KBirth, KBundle, Language, Program};
use crate::scheduler::NativeStep;
use crate::scope::{BodyShape, ShapeError};
use crate::symbols::{BinderSymbol, SymbolInterner};
use crate::type_lattice::{KType, TypeRegistry};

/// Koan's language: what [`CellSubstrate::load`](crate::program::CellSubstrate::load) runs a
/// program under.
pub struct Koan;

impl Language for Koan {
    fn builtins<'graph>(
        writer: Writer<'graph>,
        symbols: &'graph SymbolInterner,
        types: &'graph TypeRegistry<'graph>,
        scratch: BumpAllocator<'_>,
    ) -> &'graph KBuiltins<'graph, 'graph> {
        builtins::table(writer, symbols, types, scratch)
    }

    fn evaluator<'graph>() -> NativeStep<'graph, KBundle> {
        evaluate::evaluate
    }

    fn check<'graph>(
        shape: &'graph BodyShape<'graph>,
        builtins: &'graph KBuiltins<'graph, 'graph>,
        types: &'graph TypeRegistry<'graph>,
        writer: Writer<'graph>,
        scratch: BumpAllocator<'_>,
    ) -> Result<(), ShapeError<'graph>> {
        check::overlaps(shape, builtins, types, scratch)?;
        statics::statics(shape, builtins, types, writer, scratch)
    }
}

/// What one evaluation is born over: the program, the view it reads names through, the contract
/// it owes a frame it finishes, and the birth it parks with.
#[derive(Clone, Copy)]
struct Evaluation<'graph, 'here> {
    program: &'graph Program<'graph>,
    view: KActivationView<'graph, 'here>,
    contract: Option<Contract>,
    birth: KBirth<'graph, 'here>,
}

/// What one of a call's operands came to: a value, or — an `ATTR` label written bare — the name
/// it is written as.
#[derive(Clone, Copy)]
enum Operand<'graph, 'here> {
    Value(KValue<'graph, 'here>),
    Label(BinderSymbol),
}

impl<'graph, 'here> Operand<'graph, 'here> {
    /// The type a candidate's slot admits the operand by: a value's carried type, or the code kind
    /// of the name a bare label is.
    fn ktype(&self) -> KType {
        match self {
            Operand::Value(value) => value.concrete_ktype(),
            Operand::Label(BinderSymbol::Type(_)) => KType::TYPE_NAME_TOKEN,
            Operand::Label(_) => KType::IDENTIFIER,
        }
    }

    fn value(&self) -> Option<KValue<'graph, 'here>> {
        match self {
            Operand::Value(value) => Some(*value),
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
