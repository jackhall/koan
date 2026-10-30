//! The messages of the error values dispatch raises, each rendered into the region the error value
//! is built in. A refusal the layers below already render — a construction, a type expression, a
//! code shape, an `EVAL` — keeps its own wording; this file words what only dispatch decides.

use std::fmt;

use crate::knot::module::coerce::CoercionRefused;
use crate::knot::{ComposeRefused, KValue, UsingRefused};
use crate::memory::Writer;
use crate::parse::{KExpression, KeyElement};
use crate::program::Program;
use crate::scope::{ViewRefused, selection_refused, view_refused};
use crate::symbols::{KeySymbol, Symbol, SymbolInterner};
use crate::type_lattice::{KType, display_name};
use crate::values::{Chain, KeyRejected};

/// Why dispatch raised an error value.
#[derive(Clone, Copy)]
pub(super) enum Raised<'a> {
    /// No candidate at `key` admits the arguments' types.
    NoOverload {
        key: &'a [KeyElement],
        arguments: &'a [KType],
    },
    /// `count` candidates at `key` admit the arguments, none ranks first, and no builtin is among
    /// them.
    Ambiguous {
        key: &'a [KeyElement],
        arguments: &'a [KType],
        count: usize,
    },
    NoField {
        of: KType,
        field: Symbol,
    },
    NoMember {
        of: KType,
        member: Symbol,
    },
    Incomparable {
        left: KType,
        right: KType,
    },
    /// `:|` over a value that is no module.
    NotAModule {
        value: KType,
    },
    /// A module ascribed a type that names no one application of a signature.
    NotASignature {
        ascribed: KType,
    },
    /// A member an ascription's view could not take at the type the view declares for it.
    Coercion {
        name: Symbol,
        refused: CoercionRefused,
    },
    /// A quantified member read where the load recorded no type to instantiate it at.
    QuantifiedMember {
        name: Symbol,
    },
    /// A quantified member behind a view's barrier, whose type names a carrier, read anywhere but
    /// a call's head. The load refuses the read, so this stands behind it.
    BarrierInstance {
        name: Symbol,
    },
    /// A value `:!` checks against a type it does not satisfy, or a module its signature.
    Unascribable {
        value: KType,
        ascribed: KType,
    },
    /// An `EVAL` whose operand is no code.
    NotCode {
        value: KType,
    },
    /// A quote holding a splice that could not compose where it was born.
    Uncomposed {
        refused: ComposeRefused,
    },
    /// A `USING` whose module ranks `key` other than the code's own candidates do.
    RankedTwice {
        key: KeySymbol,
    },
    NotAKey {
        rejected: KeyRejected,
    },
    /// A spread, `..value`, of a value that is no list.
    NotAList {
        value: KType,
    },
    /// A list or a string applied to what is neither a `Number` nor a slice record naming `from` or
    /// `to` as `Number`s.
    Unreadable {
        head: KType,
        argument: KType,
    },
    /// A node dispatch has no reading for.
    Unevaluable {
        node: &'a KExpression<'a>,
    },
}

impl Raised<'_> {
    /// The error value carrying this message, built in `writer`'s region, its types in `types`.
    pub(super) fn raise<'graph, 'cell>(
        self,
        program: &Program<'graph>,
        writer: Writer<'cell, '_>,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> KValue<'graph, 'cell> {
        program.error(
            writer,
            types,
            RaisedDisplay {
                raised: self,
                symbols: program.symbols(),
                types,
            },
        )
    }
}

/// A [`Raised`] beside the interner and registry it renders through.
struct RaisedDisplay<'a, 'x, 'run, 'p, 'reg> {
    raised: Raised<'a>,
    symbols: &'x SymbolInterner,
    types: &'x Chain<'run, 'p, 'reg, 'x>,
}

impl fmt::Display for RaisedDisplay<'_, '_, '_, '_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ktype = |handle: KType| display_name(handle, self.types, self.symbols);
        match self.raised {
            Raised::NoOverload { key, arguments } => {
                selection_refused(f, key, arguments, None, self.symbols, self.types)
            }
            Raised::Ambiguous {
                key,
                arguments,
                count,
            } => selection_refused(f, key, arguments, Some(count), self.symbols, self.types),
            Raised::NoField { of, field } => write!(
                f,
                "{} has no field {}",
                ktype(of),
                self.symbols.display(field)
            ),
            Raised::NoMember { of, member } => write!(
                f,
                "{} has no member {}",
                ktype(of),
                self.symbols.display(member)
            ),
            Raised::Incomparable { left, right } => {
                write!(f, "{} and {} cannot be compared", ktype(left), ktype(right))
            }
            Raised::NotAModule { value } => {
                view_refused(f, ViewRefused::NotAModule(value), self.symbols, self.types)
            }
            Raised::NotASignature { ascribed } => view_refused(
                f,
                ViewRefused::NotASignature(ascribed),
                self.symbols,
                self.types,
            ),
            Raised::Coercion { name, refused } => write!(
                f,
                "member {} cannot take its view's type: {}",
                self.symbols.display(name),
                refused.display(self.types, self.symbols)
            ),
            Raised::QuantifiedMember { name } => write!(
                f,
                "member {} is quantified, and no type was known to instantiate it at",
                self.symbols.display(name)
            ),
            Raised::BarrierInstance { name } => write!(
                f,
                "member {} is quantified behind its view's barrier, and is read only at a call's head",
                self.symbols.display(name)
            ),
            Raised::Unascribable { value, ascribed } => write!(
                f,
                "{} does not satisfy its ascription {}",
                ktype(value),
                ktype(ascribed)
            ),
            Raised::NotCode { value } => {
                write!(f, "{} is not code for `EVAL` to run", ktype(value))
            }
            Raised::Uncomposed { refused } => match refused {
                ComposeRefused::NotAList { value } => {
                    write!(
                        f,
                        "`$..` splices a list of code, and {} is none",
                        ktype(value)
                    )
                }
                ComposeRefused::NotCode { element } => {
                    write!(f, "a spliced element is no code: {}", ktype(element))
                }
                ComposeRefused::TooDeep => write!(
                    f,
                    "composed code nests deeper than the syntax depth limit of {}",
                    crate::parse::MAX_SYNTAX_DEPTH
                ),
            },
            Raised::RankedTwice { key } => {
                write!(f, "{}", UsingRefused::Ranking { key }.display(self.symbols))
            }
            Raised::NotAKey { rejected } => match rejected {
                KeyRejected::NotAScalar(handle) => {
                    write!(f, "{} cannot be a dict key", ktype(handle))
                }
                KeyRejected::NaN => f.write_str("NaN cannot be a dict key"),
            },
            Raised::NotAList { value } => {
                write!(f, "`..` spreads a list, and {} is none", ktype(value))
            }
            Raised::Unreadable { head, argument } => write!(
                f,
                "{} is read by a `Number` or by a slice record naming `from` or `to` as `Number`s, \
                 not by {}",
                ktype(head),
                ktype(argument)
            ),
            Raised::Unevaluable { node } => {
                write!(f, "nothing evaluates {}", node.summary(self.symbols))
            }
        }
    }
}
