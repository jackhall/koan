//! The program record: what a loaded program's steps read at `'graph`, and what the embedder above
//! `program` supplies for it — the builtin table, the load-time check, the step every evaluation
//! runs, and the sinks a program's output and uncaught error are written to.
//!
//! An **error value** is `Error` over `{message = "<text>"}`: [`Program::error`] builds one, and
//! [`Program::message`] tells one from any other value. A [`Contract`] is what a tail hands the
//! evaluation that finishes a frame: the frame's substituted return, which
//! [`Program::fulfilled`] checks a value against and retypes it to.

use std::cell::Cell;
use std::fmt;

use crate::elaborate::builtin_error;
use crate::knot::{KActivationView, KBuiltins, KValue};
use crate::memory::{Bump, BumpAllocator, Writer};
use crate::parse::{ExpressionPart, KExpression, ParseError};
use crate::scheduler::{NativeStep, Work};
use crate::scope::{BodyShape, ShapeError, Slot};
use crate::symbols::{BinderSymbol, SymbolInterner};
use crate::type_lattice::{KType, TypeRegistry, display_name};
use crate::values::{Record, Tagged, Value, satisfies};

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

    /// What the language refuses of a shape once it is built, and what it records on it through
    /// `writer`, before anything runs — the checks that need types. None by default.
    fn check<'graph>(
        shape: &'graph BodyShape<'graph>,
        builtins: &'graph KBuiltins<'graph, 'graph>,
        types: &'graph TypeRegistry<'graph>,
        writer: Writer<'graph>,
        scratch: BumpAllocator<'_>,
    ) -> Result<(), ShapeError<'graph>> {
        let _ = (shape, builtins, types, writer, scratch);
        Ok(())
    }
}

/// Where a program's output goes: `print` takes what `PRINT` renders, and `error` the message of
/// the error that ended the program. The embedder supplies both at load.
#[derive(Clone, Copy)]
pub struct Output {
    pub print: fn(&str),
    pub error: fn(&str),
}

/// How a program's run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    /// A top-level unit received an error value, which was written to the error sink.
    Uncaught,
}

/// What the evaluation finishing a frame owes it: a value satisfying `returns`, the frame's
/// declared return with its own type-parameter solution substituted, retyped to it. `callee` is
/// the frame's function type, which a miss names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contract {
    pub callee: KType,
    pub returns: KType,
}

/// How a call reached its callee. A keyworded call's arguments were admitted by the selection that
/// chose the callee, which also put each type parameter's solution in the record by name. A call by
/// name's were written by the caller, so the frame admits them and solves the group itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    Keyworded,
    ByName,
}

/// What the body runner hands an evaluator: a whole statement, or one part of one.
#[derive(Clone, Copy, Debug)]
pub enum Evaluated<'graph> {
    Statement(&'graph KExpression<'graph>),
    Part(&'graph ExpressionPart<'graph>),
}

/// A loaded program's facts, laid down in program storage: its shape, its builtin table, the
/// registry and interner its names and types live in, the step that evaluates a node, the sinks it
/// writes to, the builtin `Error` type, and whether its last run ended uncaught. `Copy` and without
/// drop glue, so it rests in program storage beside the table it names.
#[derive(Clone, Copy)]
pub struct Program<'graph> {
    shape: &'graph BodyShape<'graph>,
    builtins: &'graph KBuiltins<'graph, 'graph>,
    types: &'graph TypeRegistry<'graph>,
    symbols: &'graph SymbolInterner,
    evaluator: NativeStep<'graph, KBundle>,
    output: Output,
    error: KType,
    /// Set by the top level when a unit receives an error value; laid down in program storage, and
    /// a `Cell<bool>` has no drop glue.
    uncaught: &'graph Cell<bool>,
}

impl<'graph> Program<'graph> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        shape: &'graph BodyShape<'graph>,
        builtins: &'graph KBuiltins<'graph, 'graph>,
        types: &'graph TypeRegistry<'graph>,
        symbols: &'graph SymbolInterner,
        evaluator: NativeStep<'graph, KBundle>,
        output: Output,
        uncaught: &'graph Cell<bool>,
    ) -> Self {
        Program {
            shape,
            builtins,
            types,
            symbols,
            evaluator,
            output,
            error: builtin_error(types, symbols, &Bump::new()),
            uncaught,
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

    pub fn output(&self) -> Output {
        self.output
    }

    /// The builtin nominal `Error`, whose representation is `{message :Str}`.
    pub fn error_type(&self) -> KType {
        self.error
    }

    pub(super) fn uncaught(&self) -> &'graph Cell<bool> {
        self.uncaught
    }

    /// The one door every evaluation is asked through: `node`, read through `view`, as a child the
    /// drain can create — or as the successor a frame's tail hands `contract`. The caller picks its
    /// placement and its `Use`; what the node means is the evaluator's, so nothing in `program`
    /// names an expression shape.
    pub fn evaluate<'here>(
        &'graph self,
        node: Evaluated<'graph>,
        view: KActivationView<'graph, 'here>,
        contract: Option<Contract>,
    ) -> Work<'graph, 'here, KBundle> {
        Work {
            step: self.evaluator,
            state: KBirth::Evaluate {
                program: self,
                node,
                view,
                contract,
            },
        }
    }

    /// An error value carrying `message`, rendered into `writer`'s region.
    pub fn error<'cell>(
        &self,
        writer: Writer<'cell>,
        message: impl fmt::Display,
    ) -> KValue<'graph, 'cell> {
        let scratch = Bump::new();
        let field = BinderSymbol::declared("message", self.symbols).expect("a value name");
        let text = Value::Str(rendered(writer, message));
        let payload = Record::new(writer, &[(field, text)], self.types, &scratch);
        Value::Tagged(Tagged::hold(writer, Value::Record(payload), self.error))
    }

    /// The message `value` carries, if it is an error value.
    pub fn message<'cell>(&self, value: &KValue<'graph, 'cell>) -> Option<&'cell str> {
        let Value::Tagged(tagged) = value else {
            return None;
        };
        if tagged.ktype() != self.error {
            return None;
        }
        let field = BinderSymbol::declared("message", self.symbols)?.symbol();
        tagged.payload().as_record()?.field(field)?.as_str()
    }

    /// `value` held to `contract`: an error value passes unchanged, a value satisfying the
    /// contract's return is retyped to it, and anything else is the error naming the miss.
    pub fn fulfilled<'cell>(
        &self,
        writer: Writer<'cell>,
        value: KValue<'graph, 'cell>,
        contract: Contract,
    ) -> KValue<'graph, 'cell> {
        if self.message(&value).is_some() {
            return value;
        }
        let scratch = Bump::new();
        if satisfies(contract.returns, &value, self.types, &scratch) {
            return value.retyped(writer, contract.returns, self.types, &scratch);
        }
        let name = |handle| display_name(handle, self.types, self.symbols);
        self.error(
            writer,
            format_args!(
                "{} returned {}, which does not satisfy {}",
                name(contract.callee),
                name(value.ktype()),
                name(contract.returns)
            ),
        )
    }

    /// The top-level slot `name` binds, if the program declares it.
    pub fn binding(&self, name: &str) -> Option<Slot> {
        let name = BinderSymbol::declared(name, self.symbols)?;
        self.shape.slot(name).map(|(slot, _)| slot)
    }
}

/// `message` rendered into `writer`'s region.
pub(super) fn rendered<'cell>(writer: Writer<'cell>, message: impl fmt::Display) -> &'cell str {
    let mut prose = writer.prose();
    fmt::Write::write_fmt(&mut prose, format_args!("{message}"))
        .expect("writing into a region does not fail");
    prose.finish()
}

/// Why a program did not load. Its `Display` is the whole diagnostic, with nothing else in hand.
#[derive(Debug)]
pub enum LoadError {
    Parse(ParseError),
    /// The shape error, rendered. The error borrows program storage and names symbols and types
    /// through the interner and registry, all of which a refused load drops, so it is rendered
    /// while they stand.
    Shape {
        rendered: String,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Parse(error) => write!(f, "{error}"),
            LoadError::Shape { rendered } => f.write_str(rendered),
        }
    }
}
