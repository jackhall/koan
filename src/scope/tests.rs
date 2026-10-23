//! Shared scaffolding for `scope`'s suites: program storage with a registry and an interner, a
//! scratch, a builtin table, and a graph to lay activations down in.

mod activation;
mod boundary;
mod code;
mod dispatch;
mod examples;
mod facts;
mod groups;
mod header;
pub(crate) mod plan;
mod properties;
mod quoted_uses;
mod quotes;
mod rewrite;
mod shapes;
mod typed;
mod units;

use crate::memory::{
    Bump, BumpAllocator, CellGraph, Edge, KnotPlan, ProgramBrand, ReleaseAbsorption, Verdict,
    Writer, covariant, program_storage, reattachable, resident,
};
use crate::parse::{KExpression, parse};
use crate::source::{FileId, SourceRef, Span};
use crate::symbols::{SymbolInterner, TypeSymbol, ValueSymbol};
use crate::type_lattice::{DeclaredType, KType, Parametric};
use crate::values::digest::{DigestHasher, Tag};
use crate::values::tests::TestTypes;
use crate::values::{Chain, ContentDigest, Seen, Types};
use crate::values::{
    DeepCopy, Knotted, KnottedFamily, MemoBits, Resolved, TypeValue, Value, ValueFamily, Weight,
};

use super::{Builtins, LoadFacts, ShapeError, Shapes};

/// A continuation family for a graph whose cells only store.
struct Step;
reattachable!(Step => ());

/// A stand-in function: the index of the knot member it is, so a read through an edge capture is
/// observable without a function layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Probe(pub u32);

impl Knotted for Probe {
    fn ktype(&self) -> DeclaredType<KType> {
        KType::ANY.into()
    }

    fn weight(&self) -> Weight {
        Weight::ZERO
    }

    /// A stand-in stores no weight, so it records no bits: a clear bit is always safe.
    fn bits(&self) -> MemoBits {
        MemoBits::NONE
    }

    fn set_bits(&self, _: MemoBits) {}

    /// A probe holds no value: every probe is a member of one empty knot, told apart by its index.
    fn held<'a>(
        &self,
        _: &Chain<'_, '_, '_, '_>,
        _: BumpAllocator<'_>,
        _: &mut dyn FnMut(Seen<'a, Self>),
    ) where
        Self: 'a,
    {
    }

    fn digest_held(
        &self,
        _: &Chain<'_, '_, '_, '_>,
        _: BumpAllocator<'_>,
        _: &mut dyn FnMut() -> ContentDigest,
    ) -> ContentDigest {
        DigestHasher::new(Tag::Knot).finished()
    }

    fn sibling(&self, edge: Edge) -> Self {
        Probe(edge.index())
    }

    /// Every probe is a member of one knot, at its own index.
    fn index(&self) -> Edge {
        KnotPlan::new(self.0 + 1)
            .edge(self.0)
            .expect("an index below its own count")
    }

    fn root(&self) -> Self {
        Probe(0)
    }

    fn resolve<'a>(&self) -> Resolved<'a, Self> {
        Resolved::Function {
            identity: self.0 as usize,
            instance: &[],
            closure: &[],
            composes: &[],
        }
    }
}

/// The family of [`Probe`], which holds no region borrow: its form is `Probe` at every brand.
pub(super) struct ProbeFamily;

impl<'graph> KnottedFamily<'graph> for ProbeFamily {
    type Closed<'cell>
        = Probe
    where
        'graph: 'cell;

    fn held<'from>(_: &Probe, _: &mut dyn FnMut(Value<'from, Probe>))
    where
        'graph: 'from,
    {
    }

    fn carried_types<'from>(_: &Probe, _: &mut dyn FnMut(DeclaredType<Parametric>))
    where
        'graph: 'from,
    {
    }

    fn copy_into<'from, 'to>(
        _: Writer<'to, '_>,
        member: &Probe,
        _: &mut DeepCopy<'_, 'from, 'to, Probe, Probe>,
    ) -> Probe
    where
        'graph: 'from,
        'graph: 'to,
    {
        *member
    }
}

// An activation over probes lays down a view of its slots, which is proven covariant here.
covariant!(ValueFamily<ProbeFamily>);

pub(super) struct Fixture<'f, 'graph> {
    pub program: ProgramBrand<'graph>,
    /// The load's chain, which every shape a test builds goes through.
    pub types: &'f Types<'graph, 'f>,
    pub symbols: &'f SymbolInterner,
    scratch: &'f Bump,
}

impl<'graph> Fixture<'_, 'graph> {
    pub fn scratch(&self) -> BumpAllocator<'_> {
        self.scratch
    }

    /// The facts of `shapes` before any walk, laid in program storage as a load lays them.
    pub fn load(&self, shapes: &'graph Shapes<'graph>) -> &'graph LoadFacts<'graph> {
        let writer = self.program.writer();
        resident(writer, LoadFacts::unwalked(writer, shapes))
    }

    /// Every top-level line of `source`, parsed into program storage.
    pub fn parse(&self, source: &str) -> Vec<KExpression<'graph>> {
        parse(self.program, self.symbols, source)
            .unwrap_or_else(|error| panic!("`{source}` parses: {error:?}"))
    }

    /// Run `step` in one cell of a graph over this fixture's storage, handing it the cell's writer
    /// and a witness that the fixture's storage outlives the cell — the writer is invariant, so an
    /// activation over a program shape takes the cell's brand only where that is known.
    pub fn in_cell<R>(
        &self,
        step: impl for<'cell> FnOnce(Writer<'cell, '_>, &'cell &'graph ()) -> R,
    ) -> R {
        let mut graph: CellGraph<'graph, Step> = CellGraph::new(1, |_| Verdict::Pin);
        let cell = graph.create(None).expect("the graph has a free slot");
        let out = graph
            .enter(cell, |context| step(context.writer(), &&()))
            .expect("a fresh cell is enterable");
        graph
            .release(cell, ReleaseAbsorption::IntoHolder)
            .expect("a cell outside its step releases");
        out
    }
}

/// Run `test` against a fresh fixture.
pub(super) fn with_fixture<R>(test: impl for<'f, 'graph> FnOnce(&Fixture<'f, 'graph>) -> R) -> R {
    let storage = program_storage();
    let program = storage.brand();
    // The lattice's tiers keep a bump of their own; program storage is the one `cellgraph` store
    // everything else here is written into.
    let registry = Bump::new();
    let tiers = TestTypes::new(&registry);
    let types = tiers.load();
    let symbols = SymbolInterner::new();
    let scratch = Bump::new();
    test(&Fixture {
        program,
        types: &types,
        symbols: &symbols,
        scratch: &scratch,
    })
}

/// The shape table of `source` over the suites' builtin table, handed to `test` beside a writer
/// into program storage, a scratch and the load's chain: for a suite outside `scope` that needs
/// shapes and names no parser.
pub(crate) fn with_shapes<R>(
    source: &str,
    test: impl for<'graph> FnOnce(
        &'graph Shapes<'graph>,
        Writer<'graph, 'graph>,
        BumpAllocator<'_>,
        &Types<'graph, '_>,
    ) -> R,
) -> R {
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        fixture.in_cell(|writer, _| {
            let table: &Builtins = builtins(fixture, writer);
            let shapes = Shapes::of_program(
                fixture.program,
                &lines,
                table,
                fixture.types,
                fixture.symbols,
                fixture.scratch(),
            )
            .unwrap_or_else(|error| panic!("`{source}` shapes: {error:?}"));
            test(
                shapes,
                fixture.program.writer(),
                fixture.scratch(),
                fixture.types,
            )
        })
    })
}

/// The value builtins every suite's table holds.
pub(super) const BUILTIN_VALUES: &[&str] = &["origin"];

/// The type builtins every suite's table holds.
pub(super) const BUILTIN_TYPES: &[&str] =
    &["Number", "Str", "Bool", "Null", "Any", "Ring", "Expression"];

/// The keys every suite's table holds an overload at: the generated plans' `ZZ`, `PRINT`, and the
/// builtin operators.
pub(super) const BUILTIN_KEYS: &[&str] = &[
    "ZZ _", "ZZ _ _", "PRINT _", "_ + _", "_ - _", "_ * _", "_ / _", "_ < _", "_ <= _", "_ > _",
    "_ >= _", "_ == _", "_ AND _", "NOT _", "| _", "& _", "_ | _", "_ & _",
];

pub(super) fn value_name(text: &str, symbols: &SymbolInterner) -> ValueSymbol {
    ValueSymbol::declared(text, symbols).expect("a value token")
}

pub(super) fn type_name(text: &str, symbols: &SymbolInterner) -> TypeSymbol {
    TypeSymbol::declared(text, symbols).expect("a Type token")
}

/// The suites' builtin table: `origin = 0`, the scalar types and `Expression`, and a `Null`
/// overload at each of [`BUILTIN_KEYS`], laid down in `writer`'s region.
pub(super) fn builtins<'graph, 'cell, X: Knotted>(
    fixture: &Fixture<'_, 'graph>,
    writer: Writer<'cell, '_>,
) -> &'cell Builtins<'cell, X> {
    let symbols = fixture.symbols;
    let values: Vec<_> = BUILTIN_VALUES
        .iter()
        .map(|name| (value_name(name, symbols), Value::Number(0.0)))
        .collect();
    let handles = [
        KType::NUMBER,
        KType::STR,
        KType::BOOL,
        KType::NULL,
        KType::ANY,
        KType::ANY,
        KType::EXPRESSION,
    ];
    let types: Vec<_> = BUILTIN_TYPES
        .iter()
        .zip(handles)
        .map(|(name, handle)| {
            let value = Value::Type(TypeValue::new(writer, handle, fixture.types));
            (type_name(name, symbols), value)
        })
        .collect();
    let overloads: Vec<_> = BUILTIN_KEYS
        .iter()
        .map(|text| (symbols.key(text).expect("a key"), Value::Null))
        .collect();
    Builtins::new(writer, fixture.scratch, &values, &types, &overloads)
}

/// The location [`unlocated`] writes, which no registered source has.
pub(super) const NOWHERE: SourceRef = SourceRef {
    span: Span { start: 0, end: 0 },
    file: FileId(u32::MAX),
};

/// `error` with every location it holds replaced by [`NOWHERE`], so a suite compares the rest of it
/// structurally and pins the locations by [`located`].
pub(super) fn unlocated(error: ShapeError) -> ShapeError {
    use ShapeError as E;
    let at = NOWHERE;
    match error {
        E::Rebind { name, .. } => E::Rebind {
            name,
            first: at,
            second: at,
        },
        E::ShadowsBuiltin { name, .. } => E::ShadowsBuiltin { name, at },
        E::Unbound { name, site, .. } => E::Unbound { name, site, at },
        E::Unfixed {
            variables, wanted, ..
        } => E::Unfixed {
            variables,
            wanted,
            at,
        },
        E::NoInstance { scheme, wanted, .. } => E::NoInstance { scheme, wanted, at },
        E::AmbiguousInstance { key, .. } => E::AmbiguousInstance { key, at },
        E::NoInstanceAtCandidates { key, .. } => E::NoInstanceAtCandidates { key, at },
        E::QuantifiedValue { .. } => E::QuantifiedValue { at },
        E::QuantifiedRead { name, site, .. } => E::QuantifiedRead { name, site, at },
        E::EagerCycle {
            members,
            definitions,
            ..
        } => E::EagerCycle {
            members,
            definitions,
            at,
        },
        E::MarkOutsideQuote { .. } => E::MarkOutsideQuote { at },
        E::Unsupported { form, .. } => E::Unsupported { form, at },
        E::Malformed { form, .. } => E::Malformed { form, at },
        E::Unsurfaced { site, .. } => E::Unsurfaced { at, site },
        E::Unchained { symbol, .. } => E::Unchained { symbol, at },
        E::MixedGroups { first, second, .. } => E::MixedGroups { first, second, at },
        E::RedeclaresGroup { symbol, .. } => E::RedeclaresGroup { symbol, at },
        E::ResultOutsidePairwise { symbol, .. } => E::ResultOutsidePairwise { symbol, at },
        E::SpellsForm { symbol, .. } => E::SpellsForm { symbol, at },
        E::Derived { symbol, .. } => E::Derived { symbol, at },
        E::Unquoted { form, part, .. } => E::Unquoted { form, part, at },
        E::Inadmissible {
            form, index, slot, ..
        } => E::Inadmissible {
            form,
            index,
            slot,
            at,
        },
        E::DictDefault { site, .. } => E::DictDefault { site, at },
        E::ClosedBucket { key, .. } => E::ClosedBucket { key, at },
        E::NoKeyword { .. } => E::NoKeyword { at },
        E::RankedDefinition { .. } => E::RankedDefinition { at },
        E::NestedBinder { .. } => E::NestedBinder { at },
        E::RankingDisagrees { key, .. } => E::RankingDisagrees { key, at },
        E::NoCandidate { key, .. } => E::NoCandidate { key, at },
        E::Overlaps { key, builtin, .. } => E::Overlaps { key, builtin, at },
        E::NoAdmittingCandidate { key, arguments, .. } => {
            E::NoAdmittingCandidate { key, arguments, at }
        }
        E::NoField { of, field, .. } => E::NoField { of, field, at },
        E::NoMember { of, member, .. } => E::NoMember { of, member, at },
        E::Unlisted { name, .. } => E::Unlisted { name, at },
        E::OpenPathInType { .. } => E::OpenPathInType { at },
        E::UnpinnedMember {
            member, parameter, ..
        } => E::UnpinnedMember {
            member,
            parameter,
            at,
        },
        E::Ambiguous {
            key,
            arguments,
            count,
            ..
        } => E::Ambiguous {
            key,
            arguments,
            count,
            at,
        },
        E::ReturnNeverSatisfied { body, returns, .. } => {
            E::ReturnNeverSatisfied { body, returns, at }
        }
        E::AscriptionNeverSatisfied {
            value, ascribed, ..
        } => E::AscriptionNeverSatisfied {
            value,
            ascribed,
            at,
        },
        E::AnnotationNeverSatisfied {
            value, annotated, ..
        } => E::AnnotationNeverSatisfied {
            value,
            annotated,
            at,
        },
        E::CallNeverSatisfied {
            callee, arguments, ..
        } => E::CallNeverSatisfied {
            callee,
            arguments,
            at,
        },
        E::NotCode { value, .. } => E::NotCode { value, at },
        E::NotAModule { value, .. } => E::NotAModule { value, at },
        E::NotASignature { ascribed, .. } => E::NotASignature { ascribed, at },
        E::EvalNeverSatisfied { code, returns, .. } => E::EvalNeverSatisfied { code, returns, at },
        E::Type { error, .. } => E::Type { error, at },
        E::RepeatedGuard { guard, .. } => E::RepeatedGuard {
            guard,
            first: at,
            at,
        },
    }
}

/// The source text a location covers.
pub(super) fn located(at: SourceRef) -> String {
    at.text()
}
