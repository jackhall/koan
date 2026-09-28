//! The builtin table dispatch loads a program over, and the natives its overloads run.
//!
//! The table binds no value name. Its type names are the lattice's builtin types and `Error`. Its
//! overloads are function values: each a [`builtin`] node over the expression shape it is
//! registered at, whose `id` is its [`Native`], held at its bucket key in the table's overload
//! run, where every keyworded use at the key lists it first among its candidates.
//!
//! The library is `PRINT`; `+ - * /` over `Number`; `< <= > >=` over `Number`, returning `Bool`;
//! `AND` and `NOT` over `Bool`; `==` over `Any`; `|` and `&` over types, as the infix pair and the
//! unary list form an operator run of them is rewritten to; and each overload of the builtin
//! expression shapes whose slots dispatch evaluates — `ATTR`, `FROM`, `EVAL` and `USING` — typed
//! as their [`BUILTIN_SHAPES`] entry types it. Every overload ranks its slots in written order.
//!
//! A native runs over operands its overload's shape admitted, so it reads each one as the type its
//! slot declares. What it refuses of a value it admitted is an error value.

use crate::elaborate::{builtin_error, builtin_shape_types, declared_field};
use crate::knot::{KBuiltins, KValue, UsingRefused, builtin, using};
use crate::memory::{Bump, BumpAllocator, BumpVec, Writer};
use crate::parse::builtin_shapes::{BUILTIN_SHAPES, BuiltinShapeId, ShapeElement};
use crate::parse::{ExpressionPart, KExpression};
use crate::program::{KBundle, eval};
use crate::scheduler::{Request, Use};
use crate::scope::{Builtins, Candidate, Offer, Site};
use crate::symbols::{BinderSymbol, KeywordSymbol, Symbol, SymbolInterner, TypeSymbol};
use crate::type_lattice::{DispatchTokenElement, KType, TypeRegistry, builtin_types, meet};
use crate::values::{Circular, List, Record, TypeValue, Value};

use super::errors::Raised;
use super::{Evaluation, Operand, check};

/// What one builtin overload runs. Its discriminant is the overload's `id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Native {
    Print,
    Add,
    Subtract,
    Multiply,
    Divide,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Not,
    Equal,
    /// `A | B`.
    Union,
    /// `| [A, B, …]`, the unary form a run of `|` is rewritten to.
    UnionOf,
    Meet,
    MeetOf,
    /// `ATTR` over a module.
    ModuleMember,
    /// `ATTR` over a record, or a tagged value over one.
    Field,
    /// `ATTR` over a type, labelled by a type name: a union's variant.
    Variant,
    /// `FROM`.
    Project,
    Eval,
    Using,
}

impl Native {
    /// Every native, at its discriminant.
    const ALL: [Native; 22] = [
        Native::Print,
        Native::Add,
        Native::Subtract,
        Native::Multiply,
        Native::Divide,
        Native::Less,
        Native::LessEqual,
        Native::Greater,
        Native::GreaterEqual,
        Native::And,
        Native::Not,
        Native::Equal,
        Native::Union,
        Native::UnionOf,
        Native::Meet,
        Native::MeetOf,
        Native::ModuleMember,
        Native::Field,
        Native::Variant,
        Native::Project,
        Native::Eval,
        Native::Using,
    ];

    /// The native a builtin node's `id` names.
    pub(super) fn of(id: u32) -> Native {
        Native::ALL[id as usize]
    }
}

/// What a slot of an operator overload takes.
#[derive(Clone, Copy)]
enum Takes {
    Is(KType),
    ListOf(KType),
}

/// One element of an operator overload's shape.
#[derive(Clone, Copy)]
enum Element {
    Keyword(&'static str),
    Slot(Takes),
}

/// An operator overload: its shape's elements, its return, and what it runs.
struct Overload {
    elements: &'static [Element],
    returns: KType,
    native: Native,
}

const NUMBER: Element = Element::Slot(Takes::Is(KType::NUMBER));
const BOOL: Element = Element::Slot(Takes::Is(KType::BOOL));
const ANY: Element = Element::Slot(Takes::Is(KType::ANY));
const TYPE: Element = Element::Slot(Takes::Is(KType::ANY_TYPE));
const TYPES: Element = Element::Slot(Takes::ListOf(KType::ANY_TYPE));

/// An infix overload whose two operands take one type.
macro_rules! binary {
    ($operand:expr, $symbol:literal, $returns:expr, $native:expr) => {
        Overload {
            elements: &[$operand, Element::Keyword($symbol), $operand],
            returns: $returns,
            native: $native,
        }
    };
}

/// The overloads written here, rather than read off a builtin expression shape's entry.
static OVERLOADS: &[Overload] = &[
    Overload {
        elements: &[Element::Keyword("PRINT"), ANY],
        returns: KType::STR,
        native: Native::Print,
    },
    binary!(NUMBER, "+", KType::NUMBER, Native::Add),
    binary!(NUMBER, "-", KType::NUMBER, Native::Subtract),
    binary!(NUMBER, "*", KType::NUMBER, Native::Multiply),
    binary!(NUMBER, "/", KType::NUMBER, Native::Divide),
    binary!(NUMBER, "<", KType::BOOL, Native::Less),
    binary!(NUMBER, "<=", KType::BOOL, Native::LessEqual),
    binary!(NUMBER, ">", KType::BOOL, Native::Greater),
    binary!(NUMBER, ">=", KType::BOOL, Native::GreaterEqual),
    binary!(BOOL, "AND", KType::BOOL, Native::And),
    Overload {
        elements: &[Element::Keyword("NOT"), BOOL],
        returns: KType::BOOL,
        native: Native::Not,
    },
    binary!(ANY, "==", KType::BOOL, Native::Equal),
    binary!(TYPE, "|", KType::ANY_TYPE, Native::Union),
    Overload {
        elements: &[Element::Keyword("|"), TYPES],
        returns: KType::ANY_TYPE,
        native: Native::UnionOf,
    },
    binary!(TYPE, "&", KType::ANY_TYPE, Native::Meet),
    Overload {
        elements: &[Element::Keyword("&"), TYPES],
        returns: KType::ANY_TYPE,
        native: Native::MeetOf,
    },
];

/// The natives of a builtin expression shape's overloads, in overload order.
fn natives_of(id: BuiltinShapeId) -> &'static [Native] {
    match id {
        BuiltinShapeId::Attribute => &[Native::ModuleMember, Native::Field, Native::Variant],
        BuiltinShapeId::Projection => &[Native::Project],
        BuiltinShapeId::Eval => &[Native::Eval],
        BuiltinShapeId::UsingCode => &[Native::Using, Native::Using],
        other => unreachable!("dispatch runs no overload of {other:?}"),
    }
}

/// Lay the builtin table down in program storage, through `writer`.
pub(super) fn table<'graph>(
    writer: Writer<'graph>,
    symbols: &'graph SymbolInterner,
    types: &'graph TypeRegistry<'graph>,
    scratch: BumpAllocator<'_>,
) -> &'graph KBuiltins<'graph, 'graph> {
    let type_value = |handle| Value::Type(TypeValue::new(writer, handle, types));
    let mut names = BumpVec::new_in(scratch);
    for (name, handle) in builtin_types() {
        names.push((symbols.record(name), type_value(handle)));
    }
    names.push((
        TypeSymbol::declared("Error", symbols).expect("a Type token"),
        type_value(builtin_error(types, symbols, scratch)),
    ));

    let mut overloads = BumpVec::new_in(scratch);
    for overload in OVERLOADS {
        let keyword = |text| KeywordSymbol::declared(text, symbols).expect("an operator keyword");
        let key = symbols.record_key(overload.elements.iter().map(|element| match element {
            Element::Keyword(text) => Some(keyword(*text)),
            Element::Slot(_) => None,
        }));
        let mut elements = BumpVec::with_capacity_in(overload.elements.len(), scratch);
        elements.extend(overload.elements.iter().map(|element| match element {
            Element::Keyword(text) => DispatchTokenElement::Keyword(keyword(*text)),
            Element::Slot(Takes::Is(slot)) => DispatchTokenElement::Slot(*slot),
            Element::Slot(Takes::ListOf(item)) => DispatchTokenElement::Slot(types.list(*item)),
        }));
        let shape = types
            .shape_type(scratch, &[], &elements, &[], overload.returns)
            .handle;
        let function = builtin(writer, shape, overload.native as u32);
        overloads.push((key, Value::Knotted(function)));
    }
    for shape in BUILTIN_SHAPES.iter().filter(|shape| shape.dispatched()) {
        let key = symbols.record_key(shape.elements.iter().map(|element| match element {
            ShapeElement::Keyword(name) => Some(symbols.record(*name)),
            ShapeElement::Slot { .. } => None,
        }));
        let handles = builtin_shape_types(shape, types, scratch);
        for (handle, native) in handles.iter().zip(natives_of(shape.id)) {
            let function = builtin(writer, *handle, *native as u32);
            overloads.push((key, Value::Knotted(function)));
        }
    }
    Builtins::new(writer, scratch, &[], &names, &overloads)
}

/// What a native came to: a value, or — an `EVAL` — the frame it asks for.
pub(super) enum Ran<'graph, 'here> {
    Value(KValue<'graph, 'here>),
    Frame(Request<'graph, 'here, KBundle>),
}

/// Run `native` over the operands its overload admitted, for the call `node` evaluated through
/// `at`, building in `writer`'s region.
pub(super) fn run<'graph, 'here>(
    native: Native,
    at: &Evaluation<'graph, 'here>,
    writer: Writer<'here>,
    node: &'graph KExpression<'graph>,
    operands: &[Operand<'graph, 'here>],
) -> Ran<'graph, 'here> {
    let program = at.program;
    let types = program.types();
    let scratch = Bump::new();
    let value = |index: usize| {
        operands[index]
            .value()
            .expect("an evaluated slot holds a value")
    };
    let number = |index| match value(index) {
        Value::Number(number) => number,
        _ => unreachable!("a `Number` slot admits numbers alone"),
    };
    let flag = |index| match value(index) {
        Value::Bool(flag) => flag,
        _ => unreachable!("a `Bool` slot admits booleans alone"),
    };
    let handle = |value: KValue<'graph, 'here>| {
        value
            .as_type()
            .expect("a type slot admits type values alone")
            .handle()
    };
    let type_value = |handle| Value::Type(TypeValue::new(writer, handle, types));
    let raise = |raised: Raised<'_>| raised.raise(program, writer);
    Ran::Value(match native {
        Native::Print => {
            let mut prose = writer.prose();
            value(0)
                .render(&mut prose, types, program.symbols(), &scratch)
                .expect("writing into a region does not fail");
            let text = prose.finish();
            (program.output().print)(text);
            Value::Str(text)
        }
        Native::Add => Value::Number(number(0) + number(1)),
        Native::Subtract => Value::Number(number(0) - number(1)),
        Native::Multiply => Value::Number(number(0) * number(1)),
        Native::Divide => Value::Number(number(0) / number(1)),
        Native::Less => Value::Bool(number(0) < number(1)),
        Native::LessEqual => Value::Bool(number(0) <= number(1)),
        Native::Greater => Value::Bool(number(0) > number(1)),
        Native::GreaterEqual => Value::Bool(number(0) >= number(1)),
        Native::And => Value::Bool(flag(0) && flag(1)),
        Native::Not => Value::Bool(!flag(0)),
        Native::Equal => {
            let (left, right) = (value(0), value(1));
            match left.equals(&right, types, &scratch) {
                Ok(equal) => Value::Bool(equal),
                Err(_) => raise(Raised::Incomparable {
                    left: left.ktype(),
                    right: right.ktype(),
                }),
            }
        }
        Native::Union => {
            type_value(types.union_of(&scratch, &[handle(value(0)), handle(value(1))]))
        }
        Native::UnionOf => {
            let mut members = BumpVec::new_in(&scratch);
            members.extend(listed(value(0)).iter().map(|item| handle(*item)));
            type_value(types.union_of(&scratch, &members))
        }
        Native::Meet => type_value(meet(types, &scratch, handle(value(0)), handle(value(1)))),
        Native::MeetOf => {
            let met = listed(value(0)).iter().fold(KType::ANY, |met, item| {
                meet(types, &scratch, met, handle(*item))
            });
            type_value(met)
        }
        Native::ModuleMember => raise(Raised::ModuleMember),
        Native::Field => {
            let (record, name) = (value(0), label(operands[1]).symbol());
            // A type's field is the type its record declares the field with, as `:(Point.y)` reads.
            if let Some(owner) = record.as_type() {
                let owner = owner.handle();
                return Ran::Value(match declared_field(types, &scratch, owner, name) {
                    Some(declared) => type_value(declared),
                    None => raise(Raised::NoField {
                        of: owner,
                        field: name,
                    }),
                });
            }
            field(record, name).unwrap_or_else(|| {
                raise(Raised::NoField {
                    of: record.ktype(),
                    field: name,
                })
            })
        }
        Native::Variant => {
            let (union, name) = (handle(value(0)), label(operands[1]).symbol());
            match types.union_member_named(union, name) {
                Some(variant) => type_value(variant),
                None => raise(Raised::NoMember {
                    of: union,
                    member: name,
                }),
            }
        }
        Native::Project => {
            let record = value(1);
            let names = listed(value(0));
            let mut fields = BumpVec::with_capacity_in(names.len(), &scratch);
            for name in names.iter() {
                let name = label(Operand::Value(*name));
                match field(record, name.symbol()) {
                    Some(value) => fields.push((name, value)),
                    None => {
                        return Ran::Value(raise(Raised::NoField {
                            of: record.ktype(),
                            field: name.symbol(),
                        }));
                    }
                }
            }
            Value::Record(Record::new(writer, &fields, types, &scratch))
        }
        Native::Eval => return evaluated(at, writer, node, value(0)),
        Native::Using => {
            let code = value(0).as_code().expect("a `Code` slot admits code alone");
            match using(writer, code, value(1), types, &scratch) {
                Ok(filled) => Value::Knotted(filled),
                Err(UsingRefused::Ranking { key }) => raise(Raised::RankedTwice { key }),
            }
        }
    })
}

/// The cells of a list a slot admitted.
fn listed<'graph, 'here>(value: KValue<'graph, 'here>) -> &'here [KValue<'graph, 'here>] {
    value
        .as_list()
        .expect("a list slot admits lists alone")
        .cells()
}

/// The name a label is: a bare one as written, or the name a one-name quote's code is.
fn label(operand: Operand<'_, '_>) -> BinderSymbol {
    let code = match operand {
        Operand::Label(name) => return name,
        Operand::Value(value) => value
            .as_code()
            .and_then(|member| member.code())
            .expect("a name slot admits a quote's code alone"),
    };
    match code.body().reference().parts {
        [only] => match only.value {
            ExpressionPart::Identifier(name) => BinderSymbol::Value(name),
            ExpressionPart::Type(name) => BinderSymbol::Type(name),
            _ => unreachable!("a name slot admits one-name code alone"),
        },
        _ => unreachable!("a name slot admits one-name code alone"),
    }
}

/// The field `name` of `value`: a record's, read through every tagged layer over it, a knot's data
/// node's included. `None` when no record holds it.
fn field<'graph, 'here>(
    value: KValue<'graph, 'here>,
    name: Symbol,
) -> Option<KValue<'graph, 'here>> {
    match value {
        Value::Record(record) => record.field(name).copied(),
        Value::Tagged(tagged) => field(*tagged.payload(), name),
        Value::Knotted(_) => match value.as_circular()? {
            (holder, Circular::Record(record)) => {
                record.field(name).map(|link| link.resolve(holder))
            }
            (holder, Circular::Tagged(tagged)) => field(tagged.payload().resolve(holder), name),
            _ => None,
        },
        _ => None,
    }
}

/// `EVAL code`: the code's shape checked for overlaps as a loaded program's is, then run in a frame
/// over the names and keys the `EVAL` offers — a key as the list of its functions, as a use at the
/// key written at the `EVAL` resolves it. A refusal is an error value.
fn evaluated<'graph, 'here>(
    at: &Evaluation<'graph, 'here>,
    writer: Writer<'here>,
    node: &'graph KExpression<'graph>,
    code: KValue<'graph, 'here>,
) -> Ran<'graph, 'here> {
    let program = at.program;
    let (types, symbols) = (program.types(), program.symbols());
    let scratch = Bump::new();
    let code = code.as_code().expect("a `Code` slot admits code alone");
    let shape = code.code().expect("a quote's code").shape();
    if let Err(error) = check::overlaps(shape, program.builtins(), types, &scratch) {
        return Ran::Value(program.error(writer, error.display(symbols, types)));
    }
    let operand = node
        .parts
        .iter()
        .find(|part| !matches!(part.value, ExpressionPart::Keyword(_)))
        .expect("`EVAL` has an operand");
    let offers = at.view.shape().offers(Site::of(&operand.value));
    let mut fields = BumpVec::with_capacity_in(offers.len(), &scratch);
    for (name, offer) in offers {
        let offered = match offer {
            Offer::Name(coordinate) => at.view.read(*coordinate),
            Offer::Key(list) => {
                let mut functions = BumpVec::new_in(&scratch);
                for candidate in list.candidates {
                    match candidate {
                        Candidate::One(coordinate) => functions.push(at.view.read(*coordinate)),
                        Candidate::Spread(coordinate) => {
                            if let Some(spread) = at.view.read(*coordinate).as_list() {
                                functions.extend(spread.cells().iter().copied());
                            }
                        }
                    }
                }
                Value::List(List::new(
                    writer,
                    functions.iter().copied(),
                    types,
                    &scratch,
                ))
            }
        };
        fields.push((*name, offered));
    }
    let offered = Value::Record(Record::new(writer, &fields, types, &scratch));
    match eval(program, code, offered, Use::Forwards) {
        Ok(request) => Ran::Frame(request),
        Err(refused) => Ran::Value(program.error(writer, refused.display(symbols, types))),
    }
}
