//! The program record: what a loaded program's steps read at `'graph`, and what the embedder above
//! `program` supplies for it — the builtin table and the step every evaluation runs.

use crate::knot::{KActivationView, KBuiltins};
use crate::memory::{BumpAllocator, Writer};
use crate::parse::{ExpressionPart, KExpression, ParseError};
use crate::scheduler::{NativeStep, Work};
use crate::scope::{BodyShape, ShapeError, Slot};
use crate::symbols::{BinderSymbol, SymbolInterner};
use crate::type_lattice::TypeRegistry;

use super::bundle::{KBirth, KBundle};

/// What the layer above `program` supplies: the builtin table a shape resolves builtin names
/// against, and the step that evaluates a node. Dispatch implements it; the tests implement a
/// miniature.
///
/// A trait rather than a record of function pointers: a pointer ranked over `'graph` cannot name
/// the bundle's projections (rustc #100013), and a method generic over `'graph` is handed the one
/// the program loads at.
pub trait Language {
    /// Lay the builtin table down at `'graph`, through the writer of program storage's store.
    fn builtins<'graph>(
        writer: Writer<'graph>,
        symbols: &'graph SymbolInterner,
        types: &'graph TypeRegistry<'graph>,
        scratch: BumpAllocator<'_>,
    ) -> &'graph KBuiltins<'graph, 'graph>;

    /// The step an evaluation runs: born holding [`KBirth::Evaluate`], it turns the node into a
    /// value delivered where the drain says.
    fn evaluator<'graph>() -> NativeStep<'graph, KBundle>;
}

/// What the body runner hands an evaluator: a whole statement, or one part of one.
#[derive(Clone, Copy, Debug)]
pub enum Evaluated<'graph> {
    Statement(&'graph KExpression<'graph>),
    Part(&'graph ExpressionPart<'graph>),
}

/// A loaded program's facts, laid down in program storage: its shape, its builtin table, the
/// registry and interner its names and types live in, and the step that evaluates a node. `Copy`
/// and without drop glue, so it rests in program storage beside the table it names.
#[derive(Clone, Copy)]
pub struct Program<'graph> {
    shape: &'graph BodyShape<'graph>,
    builtins: &'graph KBuiltins<'graph, 'graph>,
    types: &'graph TypeRegistry<'graph>,
    symbols: &'graph SymbolInterner,
    evaluator: NativeStep<'graph, KBundle>,
}

impl<'graph> Program<'graph> {
    pub(super) fn new(
        shape: &'graph BodyShape<'graph>,
        builtins: &'graph KBuiltins<'graph, 'graph>,
        types: &'graph TypeRegistry<'graph>,
        symbols: &'graph SymbolInterner,
        evaluator: NativeStep<'graph, KBundle>,
    ) -> Self {
        Program {
            shape,
            builtins,
            types,
            symbols,
            evaluator,
        }
    }

    /// The top level's shape.
    pub fn shape(&self) -> &'graph BodyShape<'graph> {
        self.shape
    }

    pub fn builtins(&self) -> &'graph KBuiltins<'graph, 'graph> {
        self.builtins
    }

    pub fn types(&self) -> &'graph TypeRegistry<'graph> {
        self.types
    }

    pub fn symbols(&self) -> &'graph SymbolInterner {
        self.symbols
    }

    /// The one door every evaluation is asked through: `node`, read through `view`, as a child the
    /// drain can create. The caller picks its placement and its `Use`; what the node means is the
    /// evaluator's, so nothing in `program` names an expression form.
    pub fn evaluate<'here>(
        &'graph self,
        node: Evaluated<'graph>,
        view: KActivationView<'graph, 'here>,
    ) -> Work<'graph, 'here, KBundle> {
        Work {
            step: self.evaluator,
            state: KBirth::Evaluate {
                program: self,
                node,
                view,
            },
        }
    }

    /// The top-level slot `name` binds, if the program declares it.
    pub fn binding(&self, name: &str) -> Option<Slot> {
        let name = BinderSymbol::declared(name, self.symbols)?;
        self.shape.slot(name).map(|(slot, _)| slot)
    }
}

/// Why a program did not load.
#[derive(Debug)]
pub enum LoadError {
    Parse(ParseError),
    Shape(ShapeError),
}
