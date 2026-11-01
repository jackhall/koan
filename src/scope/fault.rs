//! A running program's refusals: the closed set of [`Fault`] kinds, the refusal enums the layers
//! below report, which a kind carries, and the [`Link`]s a fault's way out is laid as.
//!
//! A fault holds structured fields — types, names, symbols, code, extents, and the values a raise
//! carries — and no rendered text. This module names no knot, so a fault is generic over the value
//! type it holds; `knot`'s `KFault` names koan's. [`Fault::display`] words every kind through the
//! interner and a chain, as [`ShapeError::display`] words the load's refusals; [`Named`] visits
//! every type a fault names, and [`Fault::values`] every value it holds, which a caching body's
//! exit adopts. Each kind is a [`FaultKind`], the name of its variant of the builtin `Error`. A [`Trace`] words an uncaught fault's diagnostic, its
//! message and the trace lines its links owe. The kinds live here, beside `ShapeError`, because a
//! kept cell's outermost link lies in its [region header](super::header), which is this module's.
//! See [README.md § Faults](README.md#faults).

use std::fmt;

use crate::memory::{Bump, BumpAllocator, BumpVec, Writer, collect, resident};
use crate::parse::{KExpression, KeyElement, MAX_SYNTAX_DEPTH, copied as copied_node};
use crate::source::SourceRef;
use crate::symbols::{BinderSymbol, KeySymbol, Symbol, SymbolInterner};
use crate::type_lattice::{DeclaredType, KType, Parametric, display_name};
use crate::values::{
    Chain, ConstructionRefused, KeptCell, Key, KeyRejected, Knotted, SealRefused, Value,
};

use super::record_types::{Each, Named};
use super::relay::relaid_error;
use super::shape::{
    ShapeError, ViewRefused, arm_refused, selection_refused, spelled, view_refused,
};
use super::typed::Elaboration;

/// A refusal a running program meets: one of a closed set of kinds, each over structured fields
/// and no rendered text, laid in the region it is raised in. A slice field is laid there too. A
/// field naming what a value at the raise carries holds that value, a `V`, which lies in the
/// raising region or is pinned there, so the fault keeps it where it lies.
#[derive(Clone, Copy, Debug)]
pub enum Fault<'cell, V = Value<'cell>> {
    /// No candidate at `key` admits the `arguments`, which selection read as `types` — a bare
    /// label as the code kind of its name, held among the arguments as the name's text.
    NoOverload {
        key: &'cell [KeyElement],
        arguments: &'cell [V],
        types: &'cell [KType],
    },
    /// `count` candidates at `key` admit the arguments, none ranks first, and no builtin is among
    /// them.
    AmbiguousOverloads {
        key: &'cell [KeyElement],
        arguments: &'cell [V],
        types: &'cell [KType],
        count: usize,
    },
    /// No arm of a `MATCH` admits its `scrutinee`; `union` is a `MATCH … UNDER`'s clause.
    NoArm {
        scrutinee: V,
        union: Option<KType>,
    },
    /// `count` arms of a `MATCH` admit its `scrutinee`, and none ranks first.
    AmbiguousArms {
        scrutinee: V,
        count: usize,
    },
    /// A field read off `of`, a record, or a record type value, that names no such field.
    NoField {
        of: V,
        field: Symbol,
    },
    /// A member read off `of`, a module or a union type value, that names no such member.
    NoMember {
        of: V,
        member: Symbol,
    },
    Incomparable {
        left: V,
        right: V,
    },
    /// `:|` or `USING` over a value that is no module.
    NotAModule {
        value: V,
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
        value: V,
        ascribed: KType,
    },
    /// An `EVAL` whose operand is no code.
    NotCode {
        value: V,
    },
    /// A quote holding a splice that could not compose where it was born.
    Uncomposed(ComposeRefused),
    /// A `USING` that could not fill a code's holes.
    Unfilled(UsingRefused),
    /// A dict key, `key`, that no key can be.
    NotAKey {
        key: V,
    },
    /// A spread, `..value`, of a value that is no list.
    NotAList {
        value: V,
    },
    /// A list or a string applied to what is neither a `Number` nor a slice record naming `from` or
    /// `to` as `Number`s.
    Unreadable {
        head: V,
        argument: V,
    },
    /// A node dispatch has no reading for, laid in the raising region.
    Unevaluable {
        node: &'cell KExpression<'cell>,
    },
    /// A call of `callee`, which is no function.
    NotCallable {
        callee: V,
    },
    /// A call of `callee` with `arguments` that do not name the parameters of `signature`: the
    /// type the callee shows, or the one its function behind a barrier was checked against.
    Misnamed {
        callee: V,
        signature: DeclaredType<KType>,
        arguments: V,
    },
    /// A call of `callee` with `arguments` that do not fit `signature`, refused by a barrier
    /// crossing's `refused` where there is one.
    Unfit {
        callee: V,
        signature: DeclaredType<KType>,
        arguments: V,
        refused: Option<&'cell CoercionRefused>,
    },
    /// A call of `callee` with `arguments` against which `signature`'s group has no solution.
    Unsolved {
        callee: V,
        signature: DeclaredType<KType>,
        arguments: V,
    },
    /// A frame of `callee` — `None` for an `EVAL`'s — returned `value`, which does not satisfy
    /// `returns`.
    Returned {
        callee: Option<DeclaredType<KType>>,
        value: V,
        returns: KType,
    },
    /// A `value`, bound by `LET <name> <type> = …`, that does not satisfy its annotation.
    Annotated {
        value: V,
        annotation: KType,
    },
    /// A value returned through a view's barrier that cannot take its type.
    CrossedOut(CoercionRefused),
    /// Code an `EVAL` or an untraced body refused to run.
    Unrunnable(CodeRefused),
    /// A component a tie, or a lambda a birth, refused.
    Untied(Untieable<'cell>),
    /// A construction the construction rule refused.
    Unconstructible(ConstructionRefused),
    /// A type expression that does not elaborate.
    Unelaborated(Elaboration),
    /// A build refusal the run meets — composed code's, a settlement's or an activation's — laid
    /// in the raising region.
    Refused {
        error: &'cell ShapeError<'cell>,
    },
}

/// One kind's fields as its variant of the builtin `Error` declares them, beside the `message` and
/// `frames` every variant has.
macro_rules! kinds {
    ($($kind:ident: [$($field:literal: $declared:ident),*];)*) => {
        /// A [`Fault`]'s kind, naming no field: what a `TRY` selects an arm by, and whose name is
        /// the kind's variant of the builtin `Error`
        /// ([catching](../dispatch/README.md#catching)).
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum FaultKind {
            $($kind,)*
        }

        impl FaultKind {
            /// Every kind, in [`Fault`]'s order.
            pub const ALL: [FaultKind; 32] = [$(FaultKind::$kind,)*];

            /// The kind's name: its `Error` variant's label.
            pub const fn name(self) -> &'static str {
                match self {
                    $(FaultKind::$kind => stringify!($kind),)*
                }
            }

            /// The kind's own fields, by label and declared type, without `message` and `frames`.
            pub fn fields(self) -> &'static [(&'static str, Field)] {
                match self {
                    $(FaultKind::$kind => &[$(($field, Field::$declared)),*],)*
                }
            }
        }

        impl<V> Fault<'_, V> {
            /// This fault's kind.
            pub fn kind(&self) -> FaultKind {
                match self {
                    $(Fault::$kind { .. } => FaultKind::$kind,)*
                }
            }
        }
    };
}

kinds! {
    NoOverload: ["key": Str, "arguments": ListOfAny];
    AmbiguousOverloads: ["key": Str, "arguments": ListOfAny, "count": Number];
    NoArm: ["scrutinee": Any, "union": Any];
    AmbiguousArms: ["scrutinee": Any, "count": Number];
    NoField: ["of": Any, "field": Str];
    NoMember: ["of": Any, "member": Str];
    Incomparable: ["left": Any, "right": Any];
    NotAModule: ["value": Any];
    NotASignature: ["ascribed": Type];
    Coercion: ["name": Str];
    QuantifiedMember: ["name": Str];
    BarrierInstance: ["name": Str];
    Unascribable: ["value": Any, "ascribed": Type];
    NotCode: ["value": Any];
    Uncomposed: [];
    Unfilled: [];
    NotAKey: ["key": Any];
    NotAList: ["value": Any];
    Unreadable: ["head": Any, "argument": Any];
    Unevaluable: ["node": Str];
    NotCallable: ["callee": Any];
    Misnamed: ["callee": Any, "arguments": Any];
    Unfit: ["callee": Any, "arguments": Any];
    Unsolved: ["callee": Any, "arguments": Any];
    Returned: ["callee": Str, "value": Any, "returns": Type];
    Annotated: ["value": Any, "annotation": Type];
    CrossedOut: [];
    Unrunnable: ["name": Str];
    Untied: [];
    Unconstructible: [];
    Unelaborated: [];
    Refused: [];
}

/// The declared type of one field of an `Error` variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Any,
    Str,
    Number,
    /// A type value.
    Type,
    /// `LIST OF Any`.
    ListOfAny,
}

impl<'cell, V: Copy> Fault<'cell, V> {
    /// This fault laid again through `writer`, every slice, node and refusal it borrows copied, so
    /// it outlives what it was raised over; each value is carried over as it is, so it must already
    /// outlive the writer's region. What resolves its types is the caller's to adopt.
    pub fn relaid<'to>(&self, writer: Writer<'to, '_>, scratch: BumpAllocator<'_>) -> Fault<'to, V>
    where
        V: 'to,
    {
        let values = |values: &[V]| collect(writer, values.iter().copied());
        let ktypes = |ktypes: &[KType]| collect(writer, ktypes.iter().copied());
        let key = |key: &[KeyElement]| collect(writer, key.iter().copied());
        match *self {
            Fault::NoOverload {
                key: at,
                arguments,
                types,
            } => Fault::NoOverload {
                key: key(at),
                arguments: values(arguments),
                types: ktypes(types),
            },
            Fault::AmbiguousOverloads {
                key: at,
                arguments,
                types,
                count,
            } => Fault::AmbiguousOverloads {
                key: key(at),
                arguments: values(arguments),
                types: ktypes(types),
                count,
            },
            Fault::Unevaluable { node } => Fault::Unevaluable {
                node: copied_node(writer, node),
            },
            Fault::Untied(error) => Fault::Untied(error.relaid(writer)),
            Fault::Refused { error } => Fault::Refused {
                error: relaid_error(writer, scratch, error),
            },
            Fault::NoArm { scrutinee, union } => Fault::NoArm { scrutinee, union },
            Fault::AmbiguousArms { scrutinee, count } => Fault::AmbiguousArms { scrutinee, count },
            Fault::NoField { of, field } => Fault::NoField { of, field },
            Fault::NoMember { of, member } => Fault::NoMember { of, member },
            Fault::Incomparable { left, right } => Fault::Incomparable { left, right },
            Fault::NotAModule { value } => Fault::NotAModule { value },
            Fault::NotASignature { ascribed } => Fault::NotASignature { ascribed },
            Fault::Coercion { name, refused } => Fault::Coercion { name, refused },
            Fault::QuantifiedMember { name } => Fault::QuantifiedMember { name },
            Fault::BarrierInstance { name } => Fault::BarrierInstance { name },
            Fault::Unascribable { value, ascribed } => Fault::Unascribable { value, ascribed },
            Fault::NotCode { value } => Fault::NotCode { value },
            Fault::Uncomposed(refused) => Fault::Uncomposed(refused),
            Fault::Unfilled(refused) => Fault::Unfilled(refused),
            Fault::NotAKey { key } => Fault::NotAKey { key },
            Fault::NotAList { value } => Fault::NotAList { value },
            Fault::Unreadable { head, argument } => Fault::Unreadable { head, argument },
            Fault::NotCallable { callee } => Fault::NotCallable { callee },
            Fault::Misnamed {
                callee,
                signature,
                arguments,
            } => Fault::Misnamed {
                callee,
                signature,
                arguments,
            },
            Fault::Unfit {
                callee,
                signature,
                arguments,
                refused,
            } => Fault::Unfit {
                callee,
                signature,
                arguments,
                refused: refused.map(|refused| resident(writer, *refused)),
            },
            Fault::Unsolved {
                callee,
                signature,
                arguments,
            } => Fault::Unsolved {
                callee,
                signature,
                arguments,
            },
            Fault::Returned {
                callee,
                value,
                returns,
            } => Fault::Returned {
                callee,
                value,
                returns,
            },
            Fault::Annotated { value, annotation } => Fault::Annotated { value, annotation },
            Fault::CrossedOut(refused) => Fault::CrossedOut(refused),
            Fault::Unrunnable(refused) => Fault::Unrunnable(refused),
            Fault::Unconstructible(refused) => Fault::Unconstructible(refused),
            Fault::Unelaborated(error) => Fault::Unelaborated(error),
        }
    }

    /// Hand `each` every value the fault holds, in its fields' order: what a caching body's exit
    /// adopts beside the types it [names](super::record_types::Named).
    pub fn values(&self, each: &mut dyn FnMut(&V)) {
        match self {
            Fault::NoOverload { arguments, .. } | Fault::AmbiguousOverloads { arguments, .. } => {
                arguments.iter().for_each(each);
            }
            Fault::NoArm {
                scrutinee: value, ..
            }
            | Fault::AmbiguousArms {
                scrutinee: value, ..
            }
            | Fault::NoField { of: value, .. }
            | Fault::NoMember { of: value, .. }
            | Fault::NotAModule { value }
            | Fault::Unascribable { value, .. }
            | Fault::NotCode { value }
            | Fault::NotAKey { key: value }
            | Fault::NotAList { value }
            | Fault::NotCallable { callee: value }
            | Fault::Returned { value, .. }
            | Fault::Annotated { value, .. } => each(value),
            Fault::Incomparable {
                left: first,
                right: second,
            }
            | Fault::Unreadable {
                head: first,
                argument: second,
            }
            | Fault::Misnamed {
                callee: first,
                arguments: second,
                ..
            }
            | Fault::Unfit {
                callee: first,
                arguments: second,
                ..
            }
            | Fault::Unsolved {
                callee: first,
                arguments: second,
                ..
            } => {
                each(first);
                each(second);
            }
            Fault::NotASignature { .. }
            | Fault::Coercion { .. }
            | Fault::QuantifiedMember { .. }
            | Fault::BarrierInstance { .. }
            | Fault::Uncomposed(_)
            | Fault::Unfilled(_)
            | Fault::Unevaluable { .. }
            | Fault::CrossedOut(_)
            | Fault::Unrunnable(_)
            | Fault::Untied(_)
            | Fault::Unconstructible(_)
            | Fault::Unelaborated(_)
            | Fault::Refused { .. } => {}
        }
    }
}

/// One field of a fault's variant of the builtin `Error`, beside its label: what a catcher builds
/// the field from ([catching](../dispatch/README.md#catching)).
pub enum ErrorField<'a, V> {
    /// A value the raise held.
    Value(V),
    /// The values the raise held, as a list.
    Values(&'a [V]),
    /// A name, a key, a node or a callee, worded as the message words it.
    Text(&'a dyn fmt::Display),
    Number(usize),
    /// A type value, or `null` where there is none.
    Type(Option<KType>),
}

impl<'cell, V: Copy> Fault<'cell, V> {
    /// Each of the fault's own fields, in [`FaultKind::fields`] order, its text worded through
    /// `symbols` and `types`.
    pub fn each_field(
        &self,
        symbols: &SymbolInterner,
        types: &Chain<'_, '_, '_, '_>,
        each: &mut dyn FnMut(&'static str, ErrorField<'_, V>),
    ) {
        use ErrorField::{Number, Text, Type, Value, Values};
        match *self {
            Fault::NoOverload { key, arguments, .. } => {
                each("key", Text(&spelled(key, symbols)));
                each("arguments", Values(arguments));
            }
            Fault::AmbiguousOverloads {
                key,
                arguments,
                count,
                ..
            } => {
                each("key", Text(&spelled(key, symbols)));
                each("arguments", Values(arguments));
                each("count", Number(count));
            }
            Fault::NoArm { scrutinee, union } => {
                each("scrutinee", Value(scrutinee));
                each("union", Type(union));
            }
            Fault::AmbiguousArms { scrutinee, count } => {
                each("scrutinee", Value(scrutinee));
                each("count", Number(count));
            }
            Fault::NoField { of, field } => {
                each("of", Value(of));
                each("field", Text(&symbols.display(field)));
            }
            Fault::NoMember { of, member } => {
                each("of", Value(of));
                each("member", Text(&symbols.display(member)));
            }
            Fault::Incomparable { left, right } => {
                each("left", Value(left));
                each("right", Value(right));
            }
            Fault::NotAModule { value } | Fault::NotCode { value } | Fault::NotAList { value } => {
                each("value", Value(value))
            }
            Fault::NotASignature { ascribed } => each("ascribed", Type(Some(ascribed))),
            Fault::Coercion { name, .. }
            | Fault::QuantifiedMember { name }
            | Fault::BarrierInstance { name } => each("name", Text(&symbols.display(name))),
            Fault::Unascribable { value, ascribed } => {
                each("value", Value(value));
                each("ascribed", Type(Some(ascribed)));
            }
            Fault::NotAKey { key } => each("key", Value(key)),
            Fault::Unreadable { head, argument } => {
                each("head", Value(head));
                each("argument", Value(argument));
            }
            Fault::Unevaluable { node } => each("node", Text(&node.summary(symbols))),
            Fault::NotCallable { callee } => each("callee", Value(callee)),
            Fault::Misnamed {
                callee, arguments, ..
            }
            | Fault::Unfit {
                callee, arguments, ..
            }
            | Fault::Unsolved {
                callee, arguments, ..
            } => {
                each("callee", Value(callee));
                each("arguments", Value(arguments));
            }
            Fault::Returned {
                callee,
                value,
                returns,
            } => {
                match callee {
                    Some(callee) => each("callee", Text(&display_name(callee, types, symbols))),
                    None => each("callee", Text(&"`EVAL`")),
                }
                each("value", Value(value));
                each("returns", Type(Some(returns)));
            }
            Fault::Annotated { value, annotation } => {
                each("value", Value(value));
                each("annotation", Type(Some(annotation)));
            }
            Fault::Unrunnable(CodeRefused::Unbound(name) | CodeRefused::QuantifiedRead(name)) => {
                each("name", Text(&symbols.display(name.symbol())))
            }
            Fault::Uncomposed(_)
            | Fault::Unfilled(_)
            | Fault::CrossedOut(_)
            | Fault::Untied(_)
            | Fault::Unconstructible(_)
            | Fault::Unelaborated(_)
            | Fault::Refused { .. } => {}
        }
    }
}

impl<'cell, V> Fault<'cell, V> {
    /// The fault worded as its message, its names spelled through `symbols` and its types through
    /// `types`.
    pub fn display<'x, 'run, 'p, 'reg>(
        &'x self,
        symbols: &'x SymbolInterner,
        types: &'x Chain<'run, 'p, 'reg, '_>,
    ) -> FaultDisplay<'x, 'cell, 'run, 'p, 'reg, V> {
        FaultDisplay {
            fault: self,
            symbols,
            types,
        }
    }
}

/// A [`Fault`] beside the interner and registry it renders through.
pub struct FaultDisplay<'x, 'cell, 'run, 'p, 'reg, V> {
    fault: &'x Fault<'cell, V>,
    symbols: &'x SymbolInterner,
    types: &'x Chain<'run, 'p, 'reg, 'x>,
}

/// The type a read is made over: a type value's own type, which a read off a type reads, and any
/// other value's carried type.
fn read_over<X: Knotted>(value: Value<'_, X>) -> KType {
    value
        .as_type()
        .map_or_else(|| value.concrete_ktype(), |owner| owner.handle())
}

impl<X: Knotted> fmt::Display for FaultDisplay<'_, '_, '_, '_, '_, Value<'_, X>> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (symbols, types) = (self.symbols, self.types);
        let ktype = |handle: KType| display_name(handle, types, symbols);
        let name = |handle: DeclaredType<KType>| display_name(handle, types, symbols);
        let carried = |value: Value<'_, X>| ktype(value.concrete_ktype());
        let shown = |value: Value<'_, X>| name(value.ktype());
        match *self.fault {
            Fault::NoOverload { key, types: at, .. } => {
                selection_refused(f, key, at, None, symbols, types)
            }
            Fault::AmbiguousOverloads {
                key,
                types: at,
                count,
                ..
            } => selection_refused(f, key, at, Some(count), symbols, types),
            Fault::NoArm { scrutinee, union } => {
                arm_refused(f, scrutinee.concrete_ktype(), union, None, symbols, types)
            }
            Fault::AmbiguousArms { scrutinee, count } => arm_refused(
                f,
                scrutinee.concrete_ktype(),
                None,
                Some(count),
                symbols,
                types,
            ),
            Fault::NoField { of, field } => {
                let field = symbols.display(field);
                write!(f, "{} has no field {field}", ktype(read_over(of)))
            }
            Fault::NoMember { of, member } => {
                let member = symbols.display(member);
                write!(f, "{} has no member {member}", ktype(read_over(of)))
            }
            Fault::Incomparable { left, right } => {
                write!(
                    f,
                    "{} and {} cannot be compared",
                    carried(left),
                    carried(right)
                )
            }
            Fault::NotAModule { value } => {
                let refused = ViewRefused::NotAModule(value.concrete_ktype().into());
                view_refused(f, refused, symbols, types)
            }
            Fault::NotASignature { ascribed } => {
                view_refused(f, ViewRefused::NotASignature(ascribed), symbols, types)
            }
            Fault::Coercion { name, refused } => write!(
                f,
                "member {} cannot take its view's type: {}",
                symbols.display(name),
                refused.display(types, symbols)
            ),
            Fault::QuantifiedMember { name } => write!(
                f,
                "member {} is quantified, and no type was known to instantiate it at",
                symbols.display(name)
            ),
            Fault::BarrierInstance { name } => write!(
                f,
                "member {} is quantified behind its view's barrier, and is read only at a call's head",
                symbols.display(name)
            ),
            Fault::Unascribable { value, ascribed } => write!(
                f,
                "{} does not satisfy its ascription {}",
                carried(value),
                ktype(ascribed)
            ),
            Fault::NotCode { value } => {
                write!(f, "{} is not code for `EVAL` to run", carried(value))
            }
            Fault::Uncomposed(refused) => match refused {
                ComposeRefused::NotAList { value } => write!(
                    f,
                    "`$..` splices a list of code, and {} is none",
                    ktype(value)
                ),
                ComposeRefused::NotCode { element } => {
                    write!(f, "a spliced element is no code: {}", ktype(element))
                }
                ComposeRefused::TooDeep => write!(
                    f,
                    "composed code nests deeper than the syntax depth limit of {MAX_SYNTAX_DEPTH}"
                ),
            },
            Fault::Unfilled(refused) => write!(f, "{}", refused.display(symbols)),
            Fault::NotAKey { key } => {
                let rejected = Key::admits(&key).expect_err("a key no dict takes");
                key_rejected(f, rejected, symbols, types)
            }
            Fault::NotAList { value } => {
                write!(f, "`..` spreads a list, and {} is none", carried(value))
            }
            Fault::Unreadable { head, argument } => write!(
                f,
                "{} is read by a `Number` or by a slice record naming `from` or `to` as `Number`s, \
                 not by {}",
                carried(head),
                carried(argument)
            ),
            Fault::Unevaluable { node } => write!(f, "nothing evaluates {}", node.summary(symbols)),
            Fault::NotCallable { callee } => write!(f, "{} is not callable", shown(callee)),
            Fault::Misnamed {
                signature,
                arguments,
                ..
            } => write!(
                f,
                "arguments {} do not name the parameters of {}",
                shown(arguments),
                name(signature)
            ),
            Fault::Unfit {
                signature,
                arguments,
                refused,
                ..
            } => {
                write!(
                    f,
                    "{} cannot be called with {}",
                    name(signature),
                    shown(arguments)
                )?;
                match refused {
                    Some(refused) => write!(f, ": {}", refused.display(types, symbols)),
                    None => Ok(()),
                }
            }
            Fault::Unsolved {
                signature,
                arguments,
                ..
            } => write!(
                f,
                "{} cannot be solved against {}",
                name(signature),
                shown(arguments)
            ),
            Fault::Returned {
                callee: Some(callee),
                value,
                returns,
            } => write!(
                f,
                "{} returned {}, which does not satisfy {}",
                name(callee),
                shown(value),
                ktype(returns)
            ),
            Fault::Returned {
                callee: None,
                value,
                returns,
            } => write!(
                f,
                "`EVAL`'s code returned {}, which does not satisfy {}",
                shown(value),
                ktype(returns)
            ),
            Fault::Annotated { value, annotation } => write!(
                f,
                "{} does not satisfy its annotation {}",
                shown(value),
                ktype(annotation)
            ),
            Fault::CrossedOut(refused) => write!(
                f,
                "a value returned through a view cannot take its type: {}",
                refused.display(types, symbols)
            ),
            Fault::Unrunnable(refused) => write!(f, "{}", refused.display(symbols)),
            Fault::Untied(error) => write!(f, "{}", error.display(symbols, types)),
            Fault::Unconstructible(refused) => {
                write!(f, "{}", refused_construction(&refused, symbols, types))
            }
            Fault::Unelaborated(error) => write!(f, "{}", error.display(symbols, types)),
            Fault::Refused { error } => write!(f, "{}", error.display(symbols, types)),
        }
    }
}

/// Every type the fault names: its own fields', and each value's own type — a type value's, the
/// type it denotes too — which is what its message reads. What a value reaches below its own type
/// is [`Fault::values`]'s to hand on.
impl<X: Knotted> Named for Fault<'_, Value<'_, X>> {
    fn each_named(&self, each: &mut Each<'_>) {
        self.values(&mut |value| {
            each(value.ktype().into());
            if let Some(denoted) = value.as_type() {
                each(denoted.handle().into());
            }
        });
        let mut ktype = |handle: KType| each(handle.into());
        match *self {
            Fault::NoOverload { types, .. } | Fault::AmbiguousOverloads { types, .. } => {
                types.iter().copied().for_each(ktype);
            }
            Fault::NoArm { union, .. } => union.into_iter().for_each(ktype),
            Fault::NotASignature { ascribed: of }
            | Fault::Unascribable { ascribed: of, .. }
            | Fault::Uncomposed(ComposeRefused::NotAList { value: of })
            | Fault::Uncomposed(ComposeRefused::NotCode { element: of })
            | Fault::Annotated { annotation: of, .. } => ktype(of),
            Fault::Coercion { refused, .. } | Fault::CrossedOut(refused) => {
                refused.each_named(each);
            }
            Fault::Misnamed { signature, .. } | Fault::Unsolved { signature, .. } => {
                each(signature.into());
            }
            Fault::Unfit {
                signature, refused, ..
            } => {
                each(signature.into());
                refused.iter().for_each(|refused| refused.each_named(each));
            }
            Fault::Returned {
                callee, returns, ..
            } => {
                callee.iter().for_each(|callee| each((*callee).into()));
                each(returns.into());
            }
            Fault::Refused { error } => error.each_named(each),
            Fault::Untied(error) => error.each_named(each),
            Fault::Unconstructible(refused) => construction_types(refused, each),
            Fault::Unelaborated(error) => error.each_named(each),
            Fault::AmbiguousArms { .. }
            | Fault::NoField { .. }
            | Fault::NoMember { .. }
            | Fault::Incomparable { .. }
            | Fault::NotAModule { .. }
            | Fault::NotCode { .. }
            | Fault::NotAKey { .. }
            | Fault::NotAList { .. }
            | Fault::Unreadable { .. }
            | Fault::NotCallable { .. }
            | Fault::QuantifiedMember { .. }
            | Fault::BarrierInstance { .. }
            | Fault::Uncomposed(ComposeRefused::TooDeep)
            | Fault::Unfilled(_)
            | Fault::Unevaluable { .. }
            | Fault::Unrunnable(_) => {}
        }
    }
}

/// One step of a fault's way out, laid in a region it passed through: the trace line its cell
/// owes, where it owes one, and what lies inside it.
#[derive(Clone, Copy, Debug)]
pub struct Link<'cell, V = Value<'cell>> {
    pub line: Option<Line<'cell>>,
    pub inner: Inner<'cell, V>,
}

/// What lies inside a [`Link`], and what a cell that ends with a fault hands its consumer.
#[derive(Clone, Copy, Debug)]
pub enum Inner<'cell, V = Value<'cell>> {
    /// The fault itself, laid in this write home.
    Fault(&'cell Fault<'cell, V>),
    /// A link laid in this same write home: a tenant's, which shares it.
    Link(&'cell Link<'cell, V>),
    /// A kept cell below, whose region header holds the next link, beside the fault's kind.
    Below { kept: KeptCell, kind: FaultKind },
}

impl<'cell, V> Inner<'cell, V> {
    /// The fault's kind, read without reading a kept cell.
    pub fn kind(&self) -> FaultKind {
        match self {
            Inner::Fault(fault) => fault.kind(),
            Inner::Link(link) => link.inner.kind(),
            Inner::Below { kind, .. } => *kind,
        }
    }

    /// Walk the links laid in this write home, handing `each` every line outermost first, to
    /// where they end: the fault, or the kept cell below.
    pub fn walked(self, mut each: impl FnMut(&'cell Line<'cell>)) -> Bottom<'cell, V> {
        let mut link = match self {
            Inner::Link(link) => link,
            Inner::Below { kept, .. } => return Bottom::Below(kept),
            Inner::Fault(fault) => return Bottom::Fault(fault),
        };
        loop {
            if let Some(line) = &link.line {
                each(line);
            }
            link = match &link.inner {
                Inner::Link(next) => next,
                Inner::Below { kept, .. } => return Bottom::Below(*kept),
                Inner::Fault(fault) => return Bottom::Fault(fault),
            };
        }
    }
}

/// Where a walk of the links laid in one write home ends.
pub enum Bottom<'cell, V> {
    /// The fault itself.
    Fault(&'cell Fault<'cell, V>),
    /// A kept cell below, whose region header holds the next link.
    Below(KeptCell),
}

/// One frame line of a trace: the callee's type — `None` for an `EVAL`'s frame — the extent of
/// the call, and the frames tail hops replaced in its chain, most recently reached first.
#[derive(Clone, Copy, Debug)]
pub struct Line<'cell> {
    pub callee: Option<DeclaredType<KType>>,
    pub call: SourceRef,
    pub hops: &'cell [Hop],
}

/// What a chain of tail hops replaced at one callee and call site: how many frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hop {
    pub callee: Option<DeclaredType<KType>>,
    pub call: SourceRef,
    pub count: u32,
}

/// Every type the link names in the region it lies in: its line's, and what lies inside it there
/// — a kept cell below names its own in its own region.
impl<X: Knotted> Named for Link<'_, Value<'_, X>> {
    fn each_named(&self, each: &mut Each<'_>) {
        if let Some(line) = &self.line {
            line.each_named(each);
        }
        match self.inner {
            Inner::Fault(fault) => fault.each_named(each),
            Inner::Link(link) => link.each_named(each),
            Inner::Below { .. } => {}
        }
    }
}

impl Named for Line<'_> {
    fn each_named(&self, each: &mut Each<'_>) {
        let callees = self.hops.iter().map(|hop| hop.callee);
        for callee in std::iter::once(self.callee).chain(callees).flatten() {
            each(callee.into());
        }
    }
}

impl Hop {
    /// Whether `next`, gathered beside this hop, prints on its trace line: one callee at one call.
    pub fn merges(&self, next: &Hop) -> bool {
        next.callee == self.callee && next.call == self.call
    }
}

impl<'cell> Line<'cell> {
    /// The line's hops in the order a trace gathers them, outermost first: its folded table least
    /// recently reached first, then its live frame.
    pub fn hops(&self) -> impl Iterator<Item = Hop> + 'cell {
        let (live, rest) = self.folded();
        rest.rev().chain([live])
    }

    /// The line's table with its live frame folded in: the entry at the line's own callee and
    /// call, counting one frame more than the table did, and every other entry, most recently
    /// reached first.
    pub fn folded(&self) -> (Hop, impl DoubleEndedIterator<Item = Hop> + 'cell) {
        let (callee, call) = (self.callee, self.call);
        let at = move |hop: &Hop| hop.callee == callee && hop.call == call;
        let before = self
            .hops
            .iter()
            .find(|hop| at(hop))
            .map_or(0, |hop| hop.count);
        let live = Hop {
            callee,
            call,
            count: before.saturating_add(1),
        };
        (live, self.hops.iter().copied().filter(move |hop| !at(hop)))
    }
}

/// An uncaught fault's diagnostic, gathered in a scratch bump as its links are walked outermost
/// first, region by region: its message, worded from the fault, and its trace lines, each callee
/// named through the chain of the region its line lies in. Its display prints `error: ` and the
/// message, then the lines innermost first, adjacent lines at one callee and call merged and their
/// counts summed.
pub struct Trace<'x> {
    scratch: &'x Bump,
    /// Every line, outermost first: each frame's folded table, least recently reached first.
    lines: BumpVec<'x, Traced>,
    /// The message, then each line's callee as named, each a run of `text`.
    text: BumpVec<'x, u8>,
    message: Option<(usize, usize)>,
}

/// One gathered trace line: where it was reached and how many frames, and its callee as named.
#[derive(Clone, Copy)]
struct Traced {
    hop: Hop,
    named: (usize, usize),
}

impl<'x> Trace<'x> {
    /// A diagnostic gathered in `scratch`.
    pub fn new(scratch: &'x Bump) -> Self {
        Trace {
            scratch,
            lines: BumpVec::new_in(scratch),
            text: BumpVec::new_in(scratch),
            message: None,
        }
    }

    /// Gather what `inner` holds in the region `types` reads: every link's line, then the fault,
    /// worded — or the kept cell below, whose region holds the rest.
    pub fn gathered<X: Knotted>(
        &mut self,
        inner: Inner<'_, Value<'_, X>>,
        symbols: &SymbolInterner,
        types: &Chain<'_, '_, '_, '_>,
    ) -> Option<KeptCell> {
        match inner.walked(|line| self.line(line, symbols, types)) {
            Bottom::Below(kept) => Some(kept),
            Bottom::Fault(fault) => {
                let start = self.text.len();
                self.write(format_args!("{}", fault.display(symbols, types)));
                self.message = Some((start, self.text.len()));
                None
            }
        }
    }

    /// Whether no line has been gathered yet.
    pub fn is_bare(&self) -> bool {
        self.lines.is_empty()
    }

    fn line(&mut self, line: &Line<'_>, symbols: &SymbolInterner, types: &Chain<'_, '_, '_, '_>) {
        for hop in line.hops() {
            let start = self.text.len();
            match hop.callee {
                Some(callee) => {
                    self.write(format_args!("{}", display_name(callee, types, symbols)))
                }
                None => self.write(format_args!("`EVAL`")),
            }
            self.lines.push(Traced {
                hop,
                named: (start, self.text.len()),
            });
        }
    }

    /// The whole diagnostic, written once into the scratch bump.
    pub fn rendered(&self) -> &'x str {
        let mut out = BumpVec::new_in(self.scratch);
        written(&mut out, format_args!("{self}"));
        std::str::from_utf8(out.leak()).expect("written as UTF-8")
    }

    fn write(&mut self, text: fmt::Arguments<'_>) {
        written(&mut self.text, text);
    }

    fn text(&self, (start, end): (usize, usize)) -> &str {
        std::str::from_utf8(&self.text[start..end]).expect("written as UTF-8")
    }
}

/// `text` written onto the end of `out`.
fn written(out: &mut BumpVec<'_, u8>, text: fmt::Arguments<'_>) {
    struct Bytes<'a, 'x>(&'a mut BumpVec<'x, u8>);
    impl fmt::Write for Bytes<'_, '_> {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            self.0.extend_from_slice(text.as_bytes());
            Ok(())
        }
    }
    fmt::Write::write_fmt(&mut Bytes(out), text).expect("a bump takes any text");
}

/// A call's text without the parentheses that group the whole of it, where they do: a trace line
/// writes its own.
pub fn ungrouped(text: &str) -> &str {
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for (at, char) in text.char_indices() {
        match (quoted, escaped, char) {
            (true, false, '\\') => escaped = true,
            (true, _, _) if escaped => escaped = false,
            (_, _, '"') => quoted = !quoted,
            (false, _, '(') => depth += 1,
            (false, _, ')') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return match at + 1 == text.len() && text.starts_with('(') {
                        true => &text[1..at],
                        false => text,
                    };
                }
            }
            _ if depth == 0 => return text,
            _ => {}
        }
    }
    text
}

impl fmt::Display for Trace<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = self.message.expect("a walk ends at its fault");
        write!(f, "error: {}", self.text(message))?;
        let mut lines = self.lines.iter().rev().peekable();
        while let Some(line) = lines.next() {
            let mut count = line.hop.count;
            while let Some(next) = lines.next_if(|next| line.hop.merges(&next.hop)) {
                count = count.saturating_add(next.hop.count);
            }
            let (named, call) = (self.text(line.named), line.hop.call);
            let text = call.text();
            write!(f, "\n  in {named} ({}) at {call}", ungrouped(&text))?;
            if count > 1 {
                write!(f, " ×{count}")?;
            }
        }
        Ok(())
    }
}

/// Why a member could not take the view's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoercionRefused {
    /// The admission barrier refused the carrier or the witness.
    Seal(SealRefused),
    /// A slot declared a function type and the member is no function.
    NotAFunction,
    /// A slot declared a signature and the member is no module.
    NotAModule,
    /// A nested module could not take the view its slot declares.
    Nested,
    /// A union slot: no declared member's source side admits the value.
    NoUnionMember,
    /// A union slot: two members naming a head parameter the view hides admit the value, so
    /// neither says which the value is.
    TiedUnion,
    /// A declared type this walk has no arm for, or a value whose shape does not match the arm
    /// its declaration took. A **cyclic data value** lands here: a container that is a knot's data
    /// node is a knot member, not a container word, and nothing yet rebuilds one through a
    /// barrier.
    Unsupported(DeclaredType<Parametric>),
}

impl CoercionRefused {
    /// The refusal worded for a message, its types rendered through `types` and `symbols`.
    pub fn display<'a>(
        self,
        types: &'a Chain<'_, '_, '_, '_>,
        symbols: &'a SymbolInterner,
    ) -> impl fmt::Display + 'a {
        CoercionRefusedDisplay {
            refused: self,
            types,
            symbols,
        }
    }
}

impl Named for CoercionRefused {
    fn each_named(&self, each: &mut Each<'_>) {
        match *self {
            CoercionRefused::Seal(SealRefused::NotACarrier(identity)) => each(identity.into()),
            CoercionRefused::Seal(SealRefused::Misfit { carrier, witness }) => {
                each(carrier.into());
                each(witness.into());
            }
            CoercionRefused::Seal(SealRefused::NotSealed { carrier }) => each(carrier.into()),
            CoercionRefused::Unsupported(declared) => each(declared),
            CoercionRefused::NotAFunction
            | CoercionRefused::NotAModule
            | CoercionRefused::Nested
            | CoercionRefused::NoUnionMember
            | CoercionRefused::TiedUnion => {}
        }
    }
}

/// A [`CoercionRefused`] beside the registry and interner it renders through.
struct CoercionRefusedDisplay<'a, 'run, 'p, 'reg> {
    refused: CoercionRefused,
    types: &'a Chain<'run, 'p, 'reg, 'a>,
    symbols: &'a SymbolInterner,
}

impl fmt::Display for CoercionRefusedDisplay<'_, '_, '_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = |handle: KType| display_name(handle, self.types, self.symbols);
        match self.refused {
            CoercionRefused::Seal(SealRefused::NotACarrier(identity)) => {
                write!(f, "{} is no carrier", name(identity))
            }
            CoercionRefused::Seal(SealRefused::Misfit { witness, .. }) => {
                write!(f, "its value does not satisfy {}", name(witness))
            }
            CoercionRefused::Seal(SealRefused::NotSealed { carrier }) => {
                write!(f, "it is not sealed under {}", name(carrier))
            }
            CoercionRefused::NotAFunction => f.write_str("it is no function"),
            CoercionRefused::NotAModule => f.write_str("it is no module"),
            CoercionRefused::Nested => f.write_str("its module does not fit its view"),
            CoercionRefused::NoUnionMember => f.write_str("no member of its union admits it"),
            CoercionRefused::TiedUnion => {
                f.write_str("two members of its union over hidden types admit it")
            }
            CoercionRefused::Unsupported(_) => {
                f.write_str("nothing carries a value of its shape across the view")
            }
        }
    }
}

/// Why a quote's splice could not compose where the quote was born.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposeRefused {
    /// A splice's operand is no list.
    NotAList { value: KType },
    /// A spliced element is no code.
    NotCode { element: KType },
    /// The composed code nests deeper than the syntax depth limit.
    TooDeep,
}

/// Why a `USING` refused to fill a code's holes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsingRefused {
    /// The module's registrations at `key` rank its slots other than the code's own candidates at
    /// `key` do.
    Ranking { key: KeySymbol },
    /// A filled use's holder would nest the code deeper than the syntax depth limit.
    TooDeep,
}

impl UsingRefused {
    /// The refusal worded as its message, its names read through `symbols`.
    pub fn display(self, symbols: &SymbolInterner) -> impl fmt::Display + '_ {
        UsingRefusal {
            refused: self,
            symbols,
        }
    }
}

/// [`UsingRefused::display`]'s view.
struct UsingRefusal<'x> {
    refused: UsingRefused,
    symbols: &'x SymbolInterner,
}

impl fmt::Display for UsingRefusal<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.refused {
            UsingRefused::Ranking { key } => write!(
                f,
                "{} is ranked two ways",
                self.symbols.display(key.symbol())
            ),
            UsingRefused::TooDeep => write!(
                f,
                "composed code nests deeper than the syntax depth limit of {MAX_SYNTAX_DEPTH}"
            ),
        }
    }
}

/// Why an `EVAL` refused to run its code, or an untraced body its frame, beside the refusal its
/// shape kept, which is a [`Fault::Refused`].
#[derive(Clone, Copy, Debug)]
pub enum CodeRefused {
    /// A hole no `USING` filled — a name, or a key some use selects from alone — or a `\` name the
    /// `EVAL` does not offer: the first in the shape's capture order.
    Unbound(BinderSymbol),
    /// A `$` name at the entry of code built alone, bound to a quantified function and read
    /// somewhere other than a call's head.
    QuantifiedRead(BinderSymbol),
}

impl CodeRefused {
    /// The refusal worded as its message: the unbound name or key, or the quantified read.
    pub fn display<'x>(&'x self, symbols: &'x SymbolInterner) -> CodeRefusedDisplay<'x> {
        CodeRefusedDisplay {
            refused: self,
            symbols,
        }
    }
}

/// A [`CodeRefused`] beside the interner it renders through.
pub struct CodeRefusedDisplay<'x> {
    refused: &'x CodeRefused,
    symbols: &'x SymbolInterner,
}

impl fmt::Display for CodeRefusedDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.refused {
            CodeRefused::Unbound(BinderSymbol::Key(key)) => {
                write!(f, "unbound key ({})", self.symbols.display(key.symbol()))
            }
            CodeRefused::Unbound(name) => {
                write!(f, "unbound name '{}'", self.symbols.display(name.symbol()))
            }
            CodeRefused::QuantifiedRead(name) => write!(
                f,
                "`{}` is quantified, so it is read only at the head of a call; wrap it in an \
                 unquantified `FN` to pass it",
                self.symbols.display(name.symbol())
            ),
        }
    }
}

/// Why a birth refused: a component a tie could not tie, or a lambda that could not birth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Untieable<'x> {
    /// A member's signature did not elaborate.
    Type(Elaboration),
    /// A member that is neither a callable binder, nor a module binder, nor a `LET` of a value name
    /// whose right-hand side, through one-part groups, is a list, dict or record literal or a
    /// nominal construction.
    Opaque { name: BinderSymbol },
    /// Data member `name`'s right-hand side has a part at `site` the caller must evaluate first.
    Eager {
        name: BinderSymbol,
        site: super::Site,
    },
    /// A dict key in data member `name` at `site` evaluated to something no key can be.
    Key {
        name: BinderSymbol,
        site: super::Site,
        rejected: KeyRejected,
    },
    /// A construction at `site` in data member `name` the construction rule refuses.
    Construction {
        name: BinderSymbol,
        site: super::Site,
        refused: ConstructionRefused,
    },
    /// Derived nodes — containers and constructions through a family — that reach one another with
    /// no function node or cut between them, so no finite type memoizes them: the members whose
    /// right-hand sides hold them, in component order.
    TypeCycle { names: &'x [BinderSymbol] },
    /// A spread at `site` in data member `name` of what is no list: a value of another kind, a
    /// function, or a fellow member that is no list.
    NotAList {
        name: BinderSymbol,
        site: super::Site,
    },
    /// A callable's untraced body evaluated to `value`, a value of this type, which is no code.
    NotCode { value: KType },
}

impl<'x> Untieable<'x> {
    /// This refusal laid again through `writer`, its names copied.
    pub fn relaid<'to>(&self, writer: Writer<'to, '_>) -> Untieable<'to> {
        match *self {
            Untieable::TypeCycle { names } => Untieable::TypeCycle {
                names: collect(writer, names.iter().copied()),
            },
            Untieable::Type(error) => Untieable::Type(error),
            Untieable::Opaque { name } => Untieable::Opaque { name },
            Untieable::Eager { name, site } => Untieable::Eager { name, site },
            Untieable::Key {
                name,
                site,
                rejected,
            } => Untieable::Key {
                name,
                site,
                rejected,
            },
            Untieable::Construction {
                name,
                site,
                refused,
            } => Untieable::Construction {
                name,
                site,
                refused,
            },
            Untieable::NotAList { name, site } => Untieable::NotAList { name, site },
            Untieable::NotCode { value } => Untieable::NotCode { value },
        }
    }

    /// The refusal worded as its message, with its names spelled through `symbols` and its types
    /// through `types`.
    pub fn display<'d, 'run, 'p, 'reg>(
        &'d self,
        symbols: &'d SymbolInterner,
        types: &'d Chain<'run, 'p, 'reg, '_>,
    ) -> UntieableDisplay<'d, 'x, 'run, 'p, 'reg> {
        UntieableDisplay {
            error: self,
            symbols,
            types,
        }
    }
}

impl Named for Untieable<'_> {
    fn each_named(&self, each: &mut Each<'_>) {
        match *self {
            Untieable::Type(error) => error.each_named(each),
            Untieable::Key {
                rejected: KeyRejected::NotAScalar(handle),
                ..
            }
            | Untieable::NotCode { value: handle } => each(handle.into()),
            Untieable::Construction { refused, .. } => construction_types(refused, each),
            Untieable::Opaque { .. }
            | Untieable::Eager { .. }
            | Untieable::Key { .. }
            | Untieable::TypeCycle { .. }
            | Untieable::NotAList { .. } => {}
        }
    }
}

/// An [`Untieable`] beside the interner and registry it renders through.
pub struct UntieableDisplay<'d, 'x, 'run, 'p, 'reg> {
    error: &'d Untieable<'x>,
    symbols: &'d SymbolInterner,
    types: &'d Chain<'run, 'p, 'reg, 'd>,
}

impl fmt::Display for UntieableDisplay<'_, '_, '_, '_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = |name: &BinderSymbol| self.symbols.display(name.symbol());
        let ktype = |handle: &KType| display_name(*handle, self.types, self.symbols);
        match self.error {
            Untieable::Type(error) => write!(f, "{}", error.display(self.symbols, self.types)),
            Untieable::Opaque { name: member } => {
                write!(f, "`{}` is bound to nothing a knot holds", name(member))
            }
            Untieable::Eager { name: member, .. } => {
                write!(f, "`{}` needs a part evaluated first", name(member))
            }
            Untieable::Key { rejected, .. } => key_rejected(f, *rejected, self.symbols, self.types),
            Untieable::Construction { refused, .. } => write!(
                f,
                "{}",
                refused_construction(refused, self.symbols, self.types)
            ),
            Untieable::NotCode { value } => write!(
                f,
                "this body is {}, which is not code for a callable to run",
                ktype(value)
            ),
            Untieable::NotAList { name: member, .. } => {
                write!(f, "`..` in `{}` spreads what is not a list", name(member))
            }
            Untieable::TypeCycle { names } => {
                f.write_str("these bindings build values of no finite type:")?;
                for member in names.iter() {
                    write!(f, " `{}`", name(member))?;
                }
                Ok(())
            }
        }
    }
}

/// A key no dict takes, worded: what a dict literal and a tie of a data member both report.
fn key_rejected(
    f: &mut fmt::Formatter<'_>,
    rejected: KeyRejected,
    symbols: &SymbolInterner,
    types: &Chain<'_, '_, '_, '_>,
) -> fmt::Result {
    match rejected {
        KeyRejected::NotAScalar(handle) => {
            write!(
                f,
                "{} cannot be a dict key",
                display_name(handle, types, symbols)
            )
        }
        KeyRejected::NaN => f.write_str("NaN cannot be a dict key"),
    }
}

/// Every type a refused construction names.
fn construction_types(refused: ConstructionRefused, each: &mut Each<'_>) {
    match refused {
        ConstructionRefused::NotConstructible(head) => each(head.into()),
        ConstructionRefused::Misfit {
            identity,
            representation,
            payload,
        } => {
            each(identity.into());
            each(representation.into());
            each(payload.into());
        }
        ConstructionRefused::Unsolved { family, payload } => {
            each(family.into());
            each(payload.into());
        }
    }
}

/// A construction the construction rule refused, worded: what a tie of a data member and an
/// evaluated construction both report.
pub fn refused_construction<'d, 'run, 'p, 'reg>(
    refused: &'d ConstructionRefused,
    symbols: &'d SymbolInterner,
    types: &'d Chain<'run, 'p, 'reg, '_>,
) -> RefusedConstruction<'d, 'run, 'p, 'reg> {
    RefusedConstruction {
        refused,
        symbols,
        types,
    }
}

/// A [`ConstructionRefused`] beside the interner and registry it renders through.
pub struct RefusedConstruction<'d, 'run, 'p, 'reg> {
    refused: &'d ConstructionRefused,
    symbols: &'d SymbolInterner,
    types: &'d Chain<'run, 'p, 'reg, 'd>,
}

impl fmt::Display for RefusedConstruction<'_, '_, '_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ktype = |handle: &KType| display_name(*handle, self.types, self.symbols);
        match self.refused {
            ConstructionRefused::NotConstructible(head) => {
                write!(f, "{} is not callable", ktype(head))
            }
            ConstructionRefused::Misfit {
                identity,
                representation,
                payload,
            } => write!(
                f,
                "{} cannot wrap {}: its representation is {}",
                ktype(identity),
                ktype(payload),
                ktype(representation)
            ),
            ConstructionRefused::Unsolved { family, payload } => write!(
                f,
                "{} cannot be solved against {}",
                ktype(family),
                ktype(payload)
            ),
        }
    }
}
