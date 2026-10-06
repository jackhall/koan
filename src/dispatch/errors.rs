//! The messages of the error values dispatch raises, each rendered into the region the error value
//! is built in. A refusal the layers below already render — a construction, a type expression, a
//! code shape, an `EVAL` — keeps its own wording; this file words what only dispatch decides.

use std::fmt;

use crate::knot::KValue;
use crate::knot::module::coerce::CoercionRefused;
use crate::memory::Writer;
use crate::parse::{KExpression, KeyElement};
use crate::program::Program;
use crate::scope::spelled;
use crate::symbols::{KeySymbol, Symbol, SymbolInterner};
use crate::type_lattice::{KType, TypeRegistry, display_name};
use crate::values::{KeyRejected, SealRefused};

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
    /// A value `:!` checks against a type it does not satisfy, or a module its signature.
    Unascribable {
        value: KType,
        ascribed: KType,
    },
    /// An `EVAL` whose operand is no code.
    NotCode {
        value: KType,
    },
    /// A `USING` whose module ranks `key` other than the code's own candidates do.
    RankedTwice {
        key: KeySymbol,
    },
    NotAKey {
        rejected: KeyRejected,
    },
    /// A node dispatch has no reading for.
    Unevaluable {
        node: &'a KExpression<'a>,
    },
}

impl Raised<'_> {
    /// The error value carrying this message, built in `writer`'s region.
    pub(super) fn raise<'graph, 'cell>(
        self,
        program: &Program<'graph>,
        writer: Writer<'cell>,
    ) -> KValue<'graph, 'cell> {
        program.error(
            writer,
            RaisedDisplay {
                raised: self,
                symbols: program.symbols(),
                types: program.types(),
            },
        )
    }
}

/// A [`Raised`] beside the interner and registry it renders through.
struct RaisedDisplay<'a, 'x, 'run> {
    raised: Raised<'a>,
    symbols: &'x SymbolInterner,
    types: &'x TypeRegistry<'run>,
}

impl fmt::Display for RaisedDisplay<'_, '_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ktype = |handle: KType| display_name(handle, self.types, self.symbols);
        let arguments = |f: &mut fmt::Formatter<'_>, arguments: &[KType]| {
            f.write_str("(")?;
            for (index, argument) in arguments.iter().enumerate() {
                if index > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{}", ktype(*argument))?;
            }
            f.write_str(")")
        };
        match self.raised {
            Raised::NoOverload { key, arguments: of } => {
                write!(f, "no overload of {} admits ", spelled(key, self.symbols))?;
                arguments(f, of)
            }
            Raised::Ambiguous {
                key,
                arguments: of,
                count,
            } => {
                write!(
                    f,
                    "ambiguous call of {}: {count} overloads admit ",
                    spelled(key, self.symbols)
                )?;
                arguments(f, of)?;
                f.write_str(" and none ranks first")
            }
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
            Raised::NotAModule { value } => write!(f, "{} is no module to ascribe", ktype(value)),
            Raised::NotASignature { ascribed } => write!(f, "{} is no signature", ktype(ascribed)),
            Raised::Coercion { name, refused } => {
                write!(
                    f,
                    "member {} cannot take its view's type: ",
                    self.symbols.display(name)
                )?;
                match refused {
                    CoercionRefused::Seal(SealRefused::NotAMint(identity)) => {
                        write!(f, "{} is no carrier", ktype(identity))
                    }
                    CoercionRefused::Seal(SealRefused::Misfit { witness, .. }) => {
                        write!(f, "its value does not satisfy {}", ktype(witness))
                    }
                    CoercionRefused::NotAFunction => f.write_str("it is no function"),
                    CoercionRefused::NotAModule => f.write_str("it is no module"),
                    CoercionRefused::Nested => f.write_str("its module does not fit its view"),
                    CoercionRefused::NoUnionMember => {
                        f.write_str("no member of its union admits it")
                    }
                    CoercionRefused::Unsupported(_) => {
                        f.write_str("nothing carries a value of its shape across the view")
                    }
                }
            }
            Raised::QuantifiedMember { name } => write!(
                f,
                "member {} is quantified, and no type was known to instantiate it at",
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
            Raised::RankedTwice { key } => {
                write!(
                    f,
                    "{} is ranked two ways",
                    self.symbols.display(key.symbol())
                )
            }
            Raised::NotAKey { rejected } => match rejected {
                KeyRejected::NotAScalar(handle) => {
                    write!(f, "{} cannot be a dict key", ktype(handle))
                }
                KeyRejected::NaN => f.write_str("NaN cannot be a dict key"),
            },
            Raised::Unevaluable { node } => {
                write!(f, "nothing evaluates {}", node.summary(self.symbols))
            }
        }
    }
}
