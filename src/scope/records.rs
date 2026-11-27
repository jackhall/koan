//! The **load's facts**, read through one set of readers over a source of records: [`LoadFacts`]
//! over one load's own, and [`Facts`], what a step reads through its door, over the load or
//! composition a cell rests on and the instances its frame holds. Every reader takes the body's shape, so no reader names where a fact is
//! kept. A shape names its body by its segment alone, so a reader may hand in a shape
//! borrowed for any lifetime. A reader whose fact names a type takes the reading frame's chain
//! before the shape and hands back the fact with every type it names resolving there, minted from
//! its composition's term where the chain lacks it, so no read can skip the mint.
//!
//! It reads the one walk's records: one [`UnitFacts`] per unit the walk ran, each beside the
//! frontier it gathered, one [`Closing`] per owning body, and the [`Folds`] the load lays per body
//! after the walk. Every record is `Copy`, names bodies by segment and holds no cell, so each is
//! covariant. The records index every closed tree their
//! units computed by its identity, read through [`LoadFacts::tree_of`].
//!
//! A `LoadFacts` is laid in program storage before the program is. A step reads it at its own
//! lifetime through the [`Facts`] its door builds, so nothing a cell parks points into it; a value
//! born of written code keeps what it holds of its body through [`Written`], since the load is
//! never evicted.

use crate::memory::{BumpAllocator, BumpVec, Lays, Writer, collect, resident};
use crate::parse::{ExpressionPart, KExpression, copied};
use crate::source::SourceRef;
use crate::symbols::{BinderSymbol, Symbol};
use crate::type_lattice::{DeclaredType, Interval, KType, Parametric, Term, TypeDigest, Verdict};
use crate::values::{Chain, ContentDigest, Restamping, Retype, Weight};

use super::build_layer::BuildLayer;
use super::composition::Composition;
use super::derived::{Derived, DerivedName, DerivedTerm};
use super::facts::{Address, Binding, Bound, Frontier, UnitId, Unknown};
use super::keyed::Keyed;
use super::question::{Answer, Posed, Question};
use super::record_types::{Close, CloseLaying, Closed, Closer, Laying, Named};
use super::shape::{
    BodyShape, Candidate, CaptureSlot, Coordinate, Lands, Located, Segment, ShapeError, Site, Slot,
    Target,
};
use super::shapes::Shapes;
use super::site::{
    AskedBy, Passing, Payload, Selectable, SiteAnswer, SiteAnswers, SiteQuestion, SiteRule,
};
use super::tree::Tree;
use super::typed::{
    Arms, BodyClass, Caching, CandidateRoute, Elaboration, Elimination, Handoff, Holding,
    InstanceRead, Kept, MemberWanted, Narrowing, PathEntry, PathName, Pick, Point, Points,
    Rebounds, Resolution, Route, Shaped, Static, StaticCallable, StaticInstance, StaticRegistered,
    StaticType, UseStatics, Variable, ViewSlots,
};

/// What a cell reads of a typed fact where it runs ([`Facts::at_run`]).
#[derive(Clone, Copy, Debug)]
pub enum AtRun<T> {
    /// The same at every run.
    Closed(T),
    /// Left to the run's own door: a contribution read off the argument's carried type, or a
    /// view's type over a tree the binding leaves open.
    Open,
    /// Refused at every run, as the load words it.
    Refused(Elaboration),
}

/// A walk's record of one unit: its facts beside the frontier it gathered, or the mark that the
/// walk never reached it, as a quote's code refused before it. A mark hides every older record of
/// the unit, so a reader finds none, as one does where no walk reached the unit.
#[derive(Clone, Copy)]
pub enum UnitRecord<'a> {
    Typed(&'a UnitFacts<'a>, Frontier<'a>),
    Unreached,
}

impl<'a> UnitRecord<'a> {
    /// Its facts beside its frontier, where the walk reached the unit.
    pub fn typed(self) -> Option<(&'a UnitFacts<'a>, Frontier<'a>)> {
        match self {
            UnitRecord::Typed(facts, frontier) => Some((facts, frontier)),
            UnitRecord::Unreached => None,
        }
    }
}

/// One unit's facts: everything the walk computed running it.
#[derive(Clone, Copy)]
pub struct UnitFacts<'a> {
    /// The unit's own body's, and the signature of each module body it ran, by segment.
    pub typed: Keyed<'a, (Segment, Typed<'a>)>,
    /// Each callable or quote's code born here, by its segment.
    pub born: Keyed<'a, (Segment, Born<'a>)>,
    /// Each path entry this unit resolved first on its chain, by its chain root and name, beside
    /// its resolution.
    pub paths: Keyed<'a, ((Segment, PathName), (PathEntry<'a>, Resolution<'a>))>,
    /// Each lexical variable a contribution or a solution names, beside the body reading it: what
    /// the contract fold reads.
    pub named_variables: &'a [NamedVariable],
    /// An entry unit's frontier of each binder it seeds, by slot: a read of a binder's static type
    /// gathers its seed alone, and the unit's own frontier reads every seed.
    pub seeds: Keyed<'a, (Slot, Frontier<'a>)>,
    /// Each question a door posed over a run-bound name as the unit ran, in the order posed.
    pub posed: &'a [Posed<'a>],
    /// Each rule the unit decided at a site of a body it ran, stored whole, by body, site and rule.
    pub questions: Keyed<'a, SiteQuestion<'a>>,
}

/// Whether `key` reads a path class or the type a `:Module` binder carries, itself or through a
/// derived name's term (`names` finds each): a substitution by levels closes neither, so a
/// binding that keeps every question walks a layer whose keys read one.
pub fn reads_module_binders<'n>(
    types: &crate::values::Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    key: DeclaredType<Parametric>,
    names: &dyn Fn(crate::type_lattice::DerivedId) -> Option<DerivedName<'n>>,
) -> bool {
    use crate::type_lattice::Variable;
    (super::free_variables(types, scratch, key).iter()).any(|variable| match variable {
        Variable::Path { .. } => true,
        Variable::Derived { term, .. } => names(*term).is_some_and(|name| match name.term {
            DerivedTerm::Carried { .. } => true,
            term => {
                let mut reads = false;
                term.each_type(&mut |operand| {
                    reads = reads || reads_module_binders(types, scratch, operand, names);
                });
                reads
            }
        }),
        _ => false,
    })
}

/// One body's facts from one unit, each table sorted by its key: the type channel's, then the
/// value channel's, then what an inline body folds into its owner's closing.
#[derive(Clone, Copy, Debug)]
pub struct Typed<'a> {
    /// Each type binder's declaration, by slot.
    pub declared: Keyed<'a, (Slot, StaticType<'a>)>,
    /// Each registration's entry, by slot.
    pub registered: Keyed<'a, (Slot, StaticRegistered<'a>)>,
    /// Each recorded type expression's type, by site.
    pub expressions: Keyed<'a, (Site, StaticType<'a>)>,
    /// Each part's static type, by site.
    pub parts: Keyed<'a, (Site, Interval<Parametric>)>,
    /// Each statement's static type, by index into the body.
    pub statements: Keyed<'a, (u32, Interval<Parametric>)>,
    /// Each binder's static type, by slot.
    pub binders: Keyed<'a, (Slot, Holding<'a>)>,
    /// Each ascription the walk settled.
    pub settled: Keyed<'a, Site>,
    pub instances: Keyed<'a, (Site, InstanceRead<'a>)>,
    pub uses: Keyed<'a, (Site, UseStatics<'a>)>,
    /// Each selected use's closed points, by group index.
    pub selections: Keyed<'a, (Site, Points<'a>)>,
    /// Each selected use of a callable body, beside what the site fixes of its callee's names.
    pub traced: Keyed<'a, (Site, Traced<'a>)>,
    /// Each call by name's contributions, by its argument part's site.
    pub by_name: Keyed<'a, (Site, &'a [StaticType<'a>])>,
    /// Each tail call's route, by its node's site.
    pub routes: Keyed<'a, (Site, Route<'a>)>,
    /// Each `MATCH`'s arms, by its node's site.
    pub arms: Keyed<'a, (Site, Arms<'a>)>,
    /// Each retype to a closed declared type, by the site its check is settled at, and each module
    /// member read's, by its node's site.
    pub retypes: Keyed<'a, (Site, Restamp)>,
    /// Each `:|` at a closed signature, beside its view's content tree.
    pub trees: Keyed<'a, (Site, &'a Tree<'a>)>,
    /// Each `:|` over a body-born module whose content is closed, beside its keyworded members'
    /// picks.
    pub picks: Keyed<'a, (Site, &'a [Pick<'a>])>,
    /// Each module binder's content tree, by slot, where the load read one.
    pub contents: Keyed<'a, (Slot, Tree<'a>)>,
    /// The body's own minting sites.
    pub minting: Keyed<'a, Site>,
    /// Each part the walk holds in place: a call or an `EVAL` written inline whose value's type
    /// is the name its holder gives it ([`Address::Held`](crate::scope::Address::Held)), beside
    /// its content tree: what a class's variable is read through the part by.
    pub held: Keyed<'a, (Site, Tree<'a>)>,
    /// A `MODULE` or `GROUP` body's signature, written over names, where the walk typed one: the
    /// decided name on whether every member is exact, the signature where each is and `Never`
    /// where one is not.
    pub signature: Option<Parametric>,
    /// What the unit's facts give the body's class, which its closing folds: a porous exit, how many of its minting sites are views over closed trees, and
    /// the run mints of the blocks and module bodies it ran.
    pub exits: bool,
    pub closed: u32,
    pub folded: u32,
}

/// A call the walk resolved to one callable body — a keyworded use it selected the body at, or a
/// call by name whose head's tree is a closure of it — beside what the site passes at its callee's
/// slots: what the binding graph's fold reads to map the call.
#[derive(Clone, Copy, Debug)]
pub struct Traced<'a> {
    /// The callee's body.
    pub callee: Segment,
    /// Where the call is written.
    pub call: SourceRef,
    /// Whether the call is by name, kept at its head's site.
    pub by_name: bool,
    /// What the site passes at each callee slot it passes load content or a slot of its own body
    /// at, sorted by slot.
    pub fixed: &'a [(Slot, Fixed)],
    /// What the site passes at each callee slot it passes anything at, written over names, sorted
    /// by slot: each `FOR ALL` name at its point. The binding graph's call mapping.
    pub passes: &'a [(Slot, Passing)],
}

/// Load content at one of a callee's slots: what a binding binds it to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Content {
    /// A `FOR ALL` name's closed point, or a closed type value: its handle in the root.
    Type(KType),
    /// A module whose content tree is closed. `content` is its knot member's digest, which
    /// indexes its tree: a retype's bare member, a view itself. `at` is the type the argument
    /// carries into the call, `own` the member's type and `declared` the parameter's closed
    /// declared type.
    Module {
        content: ContentDigest,
        at: KType,
        own: KType,
        declared: KType,
    },
}

impl Content {
    /// Hand `each` every type this names.
    pub fn each_type(&self, each: &mut dyn FnMut(KType)) {
        match *self {
            Content::Type(kt) => each(kt),
            Content::Module {
                at, own, declared, ..
            } => {
                each(at);
                each(own);
                each(declared);
            }
        }
    }
}

/// What a call site passes at one of its callee's slots.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fixed {
    /// Load content.
    Content(Content),
    /// The caller's own slot, passed on bare.
    Passed(Slot),
}

impl Fixed {
    /// Hand `each` every type this names.
    pub fn each_type(&self, each: &mut dyn FnMut(KType)) {
        if let Fixed::Content(content) = self {
            content.each_type(each);
        }
    }
}

impl Typed<'_> {
    /// A body's facts where a unit fixed none.
    pub const EMPTY: Self = Typed {
        declared: Keyed::EMPTY,
        registered: Keyed::EMPTY,
        expressions: Keyed::EMPTY,
        parts: Keyed::EMPTY,
        statements: Keyed::EMPTY,
        binders: Keyed::EMPTY,
        settled: Keyed::EMPTY,
        instances: Keyed::EMPTY,
        uses: Keyed::EMPTY,
        selections: Keyed::EMPTY,
        traced: Keyed::EMPTY,
        by_name: Keyed::EMPTY,
        routes: Keyed::EMPTY,
        arms: Keyed::EMPTY,
        retypes: Keyed::EMPTY,
        trees: Keyed::EMPTY,
        picks: Keyed::EMPTY,
        contents: Keyed::EMPTY,
        minting: Keyed::EMPTY,
        held: Keyed::EMPTY,
        signature: None,
        exits: false,
        closed: 0,
        folded: 0,
    };
}

impl UnitFacts<'_> {
    /// A unit's record where it fixed nothing.
    pub const EMPTY: Self = UnitFacts {
        typed: Keyed::EMPTY,
        born: Keyed::EMPTY,
        paths: Keyed::EMPTY,
        named_variables: &[],
        seeds: Keyed::EMPTY,
        posed: &[],
        questions: Keyed::EMPTY,
    };
}

/// A callable or a quote's code, as the unit that births it typed it. Its registration is its
/// callable type's, and the encloser's slot holds its entry.
#[derive(Clone, Copy, Debug)]
pub struct Born<'a> {
    pub callable: StaticCallable<'a>,
    /// Its own `FOR ALL` group as its body reads it, by group order.
    pub levels: &'a [Parametric],
    /// The solution its `FOR ALL` is born instantiated at, where the walk solved one.
    pub instance: Option<StaticInstance<'a>>,
}

/// What the walk computes for an owning body once its units and deferred bodies are typed.
#[derive(Clone, Copy, Debug)]
pub struct Closing<'a> {
    pub class: Option<BodyClass>,
    /// A callable's summary: `Some(None)` where it has none, a recursive one among them.
    pub summary: Option<Option<Tree<'a>>>,
    /// Whether the callable lies in a recursive component: it never unfolds.
    pub recursive: bool,
    /// A quote's code's last statement's static type as a traced `EVAL` runs it unfilled.
    pub ran: Option<Interval<Parametric>>,
    /// Why a quote's code does not type, reported when an `EVAL` runs it.
    pub refusal: Option<&'a ShapeError<'a>>,
    /// The first registration of a quote's code that overlaps a builtin overload, reported when an
    /// `EVAL` runs it. Apart from `refusal`, since `USING` keeps code that does not type.
    pub overlap: Option<&'a ShapeError<'a>>,
    /// What a callable's frame re-stamps its arguments and its value at.
    pub retypes: Retypes<'a>,
    /// Each rule the closing's checks decided at a site of the body, stored whole, by body, site
    /// and rule, as a unit's record stores its own.
    pub questions: Keyed<'a, SiteQuestion<'a>>,
}

/// What a callable's frame re-stamps each argument and its value at, where the walk knew how: the
/// restamp to each value parameter's type, by name, and to its return.
#[derive(Clone, Copy, Debug)]
pub struct Retypes<'a> {
    pub parameters: &'a [(Symbol, Restamp)],
    pub returns: Option<Restamp>,
}

impl Retypes<'_> {
    /// A body whose frame re-stamps by no record.
    pub const NONE: Self = Retypes {
        parameters: &[],
        returns: None,
    };

    /// The retype to the parameter `name`'s type, where it is closed.
    pub fn parameter(&self, types: &Chain<'_, '_, '_, '_>, name: Symbol) -> Option<Retype> {
        (self.parameters.iter())
            .find(|(each, _)| *each == name)
            .and_then(|(_, restamp)| restamp.retype(types))
    }

    /// The retype to the return type, where it is closed.
    pub fn returned(&self, types: &Chain<'_, '_, '_, '_>) -> Option<Retype> {
        self.returns.and_then(|restamp| restamp.retype(types))
    }
}

/// How a retype to `at` re-stamps, laid where the walk knew how — `at` closed, or the rigid answer
/// over a rigid `at` — so a binding's table closes `at` as it closes any type a record
/// names, and the retype is read once it is closed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Restamp {
    pub how: Restamping,
    pub at: Parametric,
}

impl Restamp {
    /// The retype this restamp is, where `at` is closed.
    pub fn retype(&self, types: &Chain<'_, '_, '_, '_>) -> Option<Retype> {
        types.concrete(self.at).map(|at| self.how.at(at))
    }
}

/// What the load folds per body after the walk.
#[derive(Clone, Copy, Debug)]
pub struct Folds<'a> {
    pub caching: Option<&'a Caching>,
    /// A callable's or a block's parameters, or a quote's code's captures, that some unit beneath
    /// it waits on with its entry as its seam: what its frame's entry binds.
    pub waits: &'a [Address],
    /// A body's slots bound at a statement that some unit's frontier holds: where the frame running
    /// it binds again once the statement binds them.
    pub seams: &'a [Slot],
    /// The seams some path the load left open is rooted at, sorted: bound at their statement even
    /// where the load built their content, which a path reads off what the run binds.
    pub rooting: &'a [Slot],
    /// Which of the body's type captures its content composes, parallel to its layout.
    pub contract: &'a [bool],
    /// A callable's or `MODULE` body's captures past its type captures, sorted: each binder of a
    /// body around it that some unit inside it reads through no type capture, which a frame of it
    /// binds. Its content composes none: the walk's content tree of the body holds none either.
    pub binders: &'a [Unknown],
    /// The sites of the parts the body holds in place that bind at a statement of their own or
    /// their encloser's, sorted: a held part's index is its rank, the slot its activation keeps
    /// its value in.
    pub held: &'a [Site],
}

/// A lexical variable at `level` a contribution or a solution names, read from the body `reading`:
/// every callable or `MODULE` body between composes it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NamedVariable {
    pub reading: Segment,
    pub level: usize,
}

/// The fact readers over a source of records: [`LoadFacts`] over one load's, [`Facts`] over the
/// load or composition a cell rests on and the instances its frame holds. Every reader takes the body's shape and finds the fact
/// by its segment, so a reader may hand in a shape borrowed for any lifetime; one whose fact names
/// a type takes the reading frame's chain before it.
#[derive(Clone, Copy)]
pub struct FactsOver<S>(S);

/// Every fact one load decided, by body, beside the shape table they are facts of.
pub type LoadFacts<'graph> = FactsOver<LoadRecords<'graph>>;

/// What a step reads facts through, at the step's own lifetime `'step`: the load or the composition
/// its cell's chain rests on. Built per step from the cell's depot reference, so keeping one past
/// the step does not compile:
///
/// ```compile_fail
/// use koan::memory::{Writer, resident};
/// use koan::scope::Facts;
///
/// fn keep<'here>(writer: Writer<'here, '_>, facts: Facts<'_>) -> &'here Facts<'here> {
///     resident(writer, facts)
/// }
/// ```
pub type Facts<'step> = FactsOver<Layers<'step>>;

/// One load's records: its base layer, beside the shape table they are records of and each body's
/// folds.
#[derive(Clone, Copy)]
pub struct LoadRecords<'graph> {
    shapes: &'graph Shapes<'graph>,
    /// The load's base layer: binding nothing in production.
    layer: &'graph BuildLayer<'graph>,
    /// Each body, by segment, beside its type captures as its birth reads them and its folds: none
    /// before the load folds.
    bodies: &'graph [Laid<'graph>],
}

/// What a cell reads over the base its chain rests on — the load, or a composition: the base's
/// records, and what the instances its frame and its enclosers hold answer.
#[derive(Clone, Copy)]
pub struct Layers<'step> {
    base: LoadRecords<'step>,
    composition: Option<&'step Composition<'step>>,
    /// What the instances the cell's frame and its enclosers hold answer, through their handle
    /// words.
    answers: Option<&'step dyn SiteAnswers>,
    /// What writes a frame's binding into a base record's type, through the program's fact domain.
    rebinds: Option<&'step dyn super::site::Rebinds>,
    handles: super::site::Handles<'step>,
    /// What the reading frame's slots bound at its last bind site.
    binding: Option<Binding<'step>>,
}

impl<'step> Facts<'step> {
    /// The facts a cell under the load reads.
    pub fn under(load: &LoadFacts<'step>) -> Self {
        FactsOver(Layers {
            base: load.0,
            composition: None,
            answers: None,
            rebinds: None,
            handles: super::site::Handles::default(),
            binding: None,
        })
    }

    /// The facts a cell whose chain rests on `composition` reads.
    pub fn composed(composition: &'step Composition<'step>) -> Self {
        FactsOver(Layers {
            base: composition.facts().0,
            composition: Some(composition),
            answers: None,
            rebinds: None,
            handles: super::site::Handles::default(),
            binding: None,
        })
    }

    /// The composition the cell's chain rests on, if not the load.
    pub fn composition(&self) -> Option<&'step Composition<'step>> {
        self.0.composition
    }

    /// These facts answering a stored site question through `answers`, the instances the cell's
    /// frames make.
    pub fn answering(self, answers: &'step dyn SiteAnswers) -> Self {
        FactsOver(Layers {
            answers: Some(answers),
            ..self.0
        })
    }

    /// These facts writing a frame's binding into a base record's type through `rebinds`.
    pub fn rebinding(self, rebinds: &'step dyn super::site::Rebinds) -> Self {
        FactsOver(Layers {
            rebinds: Some(rebinds),
            ..self.0
        })
    }

    /// These facts as a frame holding the handle words `handles` reads them, its slots binding
    /// `binding` at its last bind site.
    pub fn framed(
        self,
        handles: super::site::Handles<'step>,
        binding: Option<Binding<'step>>,
    ) -> Self {
        FactsOver(Layers {
            handles,
            binding,
            ..self.0
        })
    }

    /// Hand `each` every term a frame reading these facts mints so every type they name resolves
    /// in its chain, in the order a term's leaves resolve: the composition's. None over the load.
    pub fn each_term(&self, each: &mut dyn FnMut(&'step Term<'step>)) {
        #[cfg(test)]
        let each = &mut |term| {
            TERM_PROBES.with(|probes| probes.set(probes.get() + 1));
            each(term)
        };
        (self.0.base.layer.terms().entries().iter()).for_each(|(_, term)| each(term));
    }

    /// Whether the base binds `unknown`.
    pub fn binds(&self, unknown: Unknown) -> bool {
        self.0.bound(unknown).is_some()
    }

    /// The refusal of the composed code the chain rests on, if it reached one, its types
    /// resolving in `types`.
    pub fn refusal(&self, types: &Chain<'_, '_, '_, '_>) -> Option<&'step ShapeError<'step>> {
        // A walk's own refusal is worded as the load words it: nothing to close.
        self.minted(types, self.0.base.layer.refusal())
    }
}

/// Where the fact readers find a record. A load's own records lie over its base's, and a re-run
/// unit's record replaces the base's whole.
pub(crate) trait RecordSource<'a>: Copy {
    fn shapes(&self) -> &'a Shapes<'a>;

    /// What `unknown` is bound to, by any layer: each layer binds its own names.
    fn bound(&self, unknown: Unknown) -> Option<Bound<'a>>;

    /// Hand `each` every unknown any layer binds, beside what it binds it to, the innermost
    /// layer's first.
    fn each_bound(&self, each: &mut dyn FnMut(Unknown, Bound<'a>));

    /// The newest record of `unit`, beside its frontier.
    fn unit(&self, unit: UnitId) -> Option<(&'a UnitFacts<'a>, Frontier<'a>)>;

    /// The newest closing record of the owning body `body`, beside its frontier.
    fn closing(&self, body: Segment) -> Option<(&'a Closing<'a>, Frontier<'a>)>;

    /// The record holding `held`, beside its unit.
    fn holding(&self, held: Held) -> Option<(UnitId, &'a UnitFacts<'a>)>;

    /// What the instance the reading frame holds answers the stored question of `rule` at `site`
    /// of `segment`: `None` where it holds none owning it.
    fn site_answer(&self, segment: Segment, site: Site, rule: SiteRule) -> Option<SiteAnswer> {
        let _ = (segment, site, rule);
        None
    }

    /// `body` as its load laid it.
    fn laid(&self, body: Segment) -> &'a Laid<'a>;

    /// The folds of `body`.
    fn folds(&self, body: Segment) -> Option<Folds<'a>>;

    /// The derived name `id`, which the innermost layer that laid it holds.
    fn derived_name(&self, id: crate::type_lattice::DerivedId) -> Option<DerivedName<'a>>;

    /// The closed tree whose identity is `digest`.
    fn tree_of(&self, digest: ContentDigest) -> Option<&'a Tree<'a>>;

    /// Whether these records rest on a composition, whose facts may name a type only a term
    /// makes in a reading frame's chain.
    fn composed(&self) -> bool {
        false
    }

    /// The term the composition laid for the type whose digest is `digest`. None over the load.
    fn term(&self, _digest: TypeDigest) -> Option<&'a Term<'a>> {
        None
    }

    /// What writes a frame's binding into a base record's type, beside the binding the reading
    /// frame's slots hold: `None` where nothing does, or the frame holds none.
    fn rebinding(&self) -> Option<(&'a dyn super::site::Rebinds, Binding<'a>)> {
        None
    }
}

impl<'graph> RecordSource<'graph> for LoadRecords<'graph> {
    fn shapes(&self) -> &'graph Shapes<'graph> {
        self.shapes
    }

    /// A load binds nothing.
    fn bound(&self, _: Unknown) -> Option<Bound<'graph>> {
        None
    }

    fn each_bound(&self, _: &mut dyn FnMut(Unknown, Bound<'graph>)) {}

    fn unit(&self, unit: UnitId) -> Option<(&'graph UnitFacts<'graph>, Frontier<'graph>)> {
        self.layer.unit(unit)?.typed()
    }

    fn closing(&self, body: Segment) -> Option<(&'graph Closing<'graph>, Frontier<'graph>)> {
        self.layer.closing(body)
    }

    fn holding(&self, held: Held) -> Option<(UnitId, &'graph UnitFacts<'graph>)> {
        self.layer.holding(held)
    }

    fn laid(&self, body: Segment) -> &'graph Laid<'graph> {
        self.bodies
            .get(body.index() as usize)
            .expect("a load folds its records before a body is read")
    }

    fn folds(&self, body: Segment) -> Option<Folds<'graph>> {
        (self.bodies.get(body.index() as usize)).and_then(|laid| laid.folds)
    }

    fn derived_name(&self, id: crate::type_lattice::DerivedId) -> Option<DerivedName<'graph>> {
        self.layer.derived().get(id).copied()
    }

    fn tree_of(&self, digest: ContentDigest) -> Option<&'graph Tree<'graph>> {
        self.layer.tree_of(digest)
    }
}

impl<'step> RecordSource<'step> for Layers<'step> {
    fn composed(&self) -> bool {
        self.composition.is_some()
    }

    fn site_answer(&self, segment: Segment, site: Site, rule: SiteRule) -> Option<SiteAnswer> {
        self.answers?.answered(segment, site, rule, self.handles)
    }

    fn rebinding(&self) -> Option<(&'step dyn super::site::Rebinds, Binding<'step>)> {
        Some((self.rebinds?, self.binding?))
    }

    fn term(&self, digest: TypeDigest) -> Option<&'step Term<'step>> {
        self.composition?;
        self.base.layer.terms().get(digest).map(|(_, term)| term)
    }

    fn shapes(&self) -> &'step Shapes<'step> {
        self.base.shapes()
    }

    fn bound(&self, unknown: Unknown) -> Option<Bound<'step>> {
        self.base.bound(unknown)
    }

    fn each_bound(&self, each: &mut dyn FnMut(Unknown, Bound<'step>)) {
        self.base.each_bound(each);
    }

    fn unit(&self, unit: UnitId) -> Option<(&'step UnitFacts<'step>, Frontier<'step>)> {
        self.base.unit(unit)
    }

    fn closing(&self, body: Segment) -> Option<(&'step Closing<'step>, Frontier<'step>)> {
        self.base.closing(body)
    }

    fn holding(&self, held: Held) -> Option<(UnitId, &'step UnitFacts<'step>)> {
        self.base.holding(held)
    }

    fn laid(&self, body: Segment) -> &'step Laid<'step> {
        self.base.laid(body)
    }

    fn folds(&self, body: Segment) -> Option<Folds<'step>> {
        self.base.folds(body)
    }

    fn derived_name(&self, id: crate::type_lattice::DerivedId) -> Option<DerivedName<'step>> {
        self.base.derived_name(id)
    }

    fn tree_of(&self, digest: ContentDigest) -> Option<&'step Tree<'step>> {
        self.base.tree_of(digest)
    }
}

// Every type the load's records name lies in the root or the agent tier, so a raw read of them
// needs no mint; a step's [`Facts`] hands a walk its records only through [`Facts::whole`].
impl<'a> RecordSource<'a> for LoadFacts<'a> {
    fn shapes(&self) -> &'a Shapes<'a> {
        self.0.shapes()
    }

    fn bound(&self, unknown: Unknown) -> Option<Bound<'a>> {
        self.0.bound(unknown)
    }

    fn each_bound(&self, each: &mut dyn FnMut(Unknown, Bound<'a>)) {
        self.0.each_bound(each);
    }

    fn unit(&self, unit: UnitId) -> Option<(&'a UnitFacts<'a>, Frontier<'a>)> {
        self.0.unit(unit)
    }

    fn closing(&self, body: Segment) -> Option<(&'a Closing<'a>, Frontier<'a>)> {
        self.0.closing(body)
    }

    fn holding(&self, held: Held) -> Option<(UnitId, &'a UnitFacts<'a>)> {
        self.0.holding(held)
    }

    fn laid(&self, body: Segment) -> &'a Laid<'a> {
        self.0.laid(body)
    }

    fn folds(&self, body: Segment) -> Option<Folds<'a>> {
        self.0.folds(body)
    }

    fn derived_name(&self, id: crate::type_lattice::DerivedId) -> Option<DerivedName<'a>> {
        self.0.derived_name(id)
    }

    fn tree_of(&self, digest: ContentDigest) -> Option<&'a Tree<'a>> {
        self.0.tree_of(digest)
    }
}

/// The code a value is born of, read at the program's own lifetime: the program's written code,
/// or, for a cell whose chain rests on a composition, the code composed at run time, under its root
/// code value, whose syntax lives at `'root`. What a value born of it keeps is its body's record or
/// a quote's syntax. The load is never evicted, so a value may point into it; a composition may be,
/// so a value born of composed code keeps its own copy of its body's record beside the root that
/// finds or rebuilds the composition, and its quote's syntax inside the root's. Every fact a step
/// decides by is read through [`Facts`] instead.
#[derive(Clone, Copy)]
pub struct Written<'graph, 'root, 'step> {
    load: LoadRecords<'graph>,
    composed: Option<(ComposedRoot<'root>, &'step Composition<'step>)>,
}

/// A body's record as a value born of it holds it: the load's, by pointer, or one of composed code
/// laid at `'a`, beside its root at `'root`.
#[derive(Clone, Copy)]
pub enum BodyAt<'graph, 'root, 'a> {
    Written(&'graph BodyRef<'graph, 'graph>),
    Composed(&'a BodyRef<'root, 'a>),
}

impl<'graph: 'root, 'root: 'a, 'a> BodyAt<'graph, 'root, 'a> {
    /// The record itself.
    pub fn get(self) -> &'a BodyRef<'root, 'a> {
        match self {
            BodyAt::Written(body) => body,
            BodyAt::Composed(body) => body,
        }
    }

    /// This record as a value copied through `writer` holds it: the load's pointer kept, composed
    /// code's record copied beside a copy of its root.
    pub fn copied<'to>(self, writer: Writer<'to, '_>) -> BodyAt<'graph, 'to, 'to>
    where
        'graph: 'to,
    {
        match self {
            BodyAt::Written(body) => BodyAt::Written(body),
            BodyAt::Composed(body) => {
                BodyAt::Composed(body.laid(writer, body.root.map(|root| root.copied(writer))))
            }
        }
    }
}

/// Where a code value's syntax is and what shapes it.
#[derive(Clone, Copy, Debug)]
pub enum CodeSyntax<'graph, 'cell> {
    /// A quote the program writes: its body in program storage, its shape the load's, by segment.
    Written {
        body: &'graph KExpression<'graph>,
        segment: Segment,
    },
    /// Code composed at run time, its syntax laid in a region: `root` finds or rebuilds its
    /// composition, and `body` is the root's syntax or a quote nested in it, whose shape `segment`
    /// names among its composition's shapes — `None` for the root itself.
    Composed {
        body: &'cell KExpression<'cell>,
        root: ComposedRoot<'cell>,
        segment: Option<Segment>,
    },
}

impl<'graph: 'cell, 'cell> CodeSyntax<'graph, 'cell> {
    /// The syntax itself.
    pub fn body(self) -> &'cell KExpression<'cell> {
        match self {
            CodeSyntax::Written { body, .. } | CodeSyntax::Composed { body, .. } => body,
        }
    }

    /// The root of the composed code this is, if it is not written.
    pub fn root(self) -> Option<ComposedRoot<'cell>> {
        match self {
            CodeSyntax::Written { .. } => None,
            CodeSyntax::Composed { root, .. } => Some(root),
        }
    }

    /// What laying this syntax again writes: nothing for written code, which a copy points at;
    /// composed code's body, and its root beside it where the body is a quote nested in it.
    pub fn weight(self) -> Weight {
        match self {
            CodeSyntax::Written { .. } => Weight::ZERO,
            CodeSyntax::Composed {
                root,
                segment: None,
                ..
            } => root.weight(),
            CodeSyntax::Composed { body, root, .. } => Weight::syntax(body).plus(root.weight()),
        }
    }

    /// This syntax as a copy through `writer` holds it: written code's pointer kept, composed
    /// code laid again, its root beside its body where the two differ.
    pub fn copied<'to>(self, writer: Writer<'to, '_>) -> CodeSyntax<'graph, 'to>
    where
        'graph: 'to,
    {
        match self {
            CodeSyntax::Written { body, segment } => CodeSyntax::Written { body, segment },
            CodeSyntax::Composed {
                root,
                segment: None,
                ..
            } => {
                let root = root.copied(writer);
                CodeSyntax::Composed {
                    body: root.syntax,
                    root,
                    segment: None,
                }
            }
            CodeSyntax::Composed {
                body,
                root,
                segment,
            } => CodeSyntax::Composed {
                body: copied(writer, body),
                root: root.copied(writer),
                segment,
            },
        }
    }
}

impl<'graph, 'root, 'step> Written<'graph, 'root, 'step> {
    /// The written code `load` holds.
    pub fn of(load: &LoadFacts<'graph>) -> Self {
        Written {
            load: load.0,
            composed: None,
        }
    }

    /// The code composed at run time whose root is `root`, which `composition` holds.
    pub fn composed(
        self,
        root: ComposedRoot<'root>,
        composition: &'step Composition<'step>,
    ) -> Self {
        Written {
            composed: Some((root, composition)),
            ..self
        }
    }

    /// The root of the composed code this is, if it is not the program's.
    pub fn root(&self) -> Option<ComposedRoot<'root>> {
        self.composed.map(|(root, _)| root)
    }

    /// The body at `segment`, as a value born of it holds it, a composed record laid through `lay`
    /// beside its root.
    pub fn body<'a>(&self, lay: impl Lays<'a>, segment: Segment) -> BodyAt<'graph, 'root, 'a>
    where
        'graph: 'a,
    {
        self.laid(lay, segment, self.root())
    }

    /// [`body`](Self::body) with no root on a composed record: what scratch shorter-lived than the
    /// root holds, the root kept beside it.
    pub fn unrooted<'a>(&self, lay: impl Lays<'a>, segment: Segment) -> BodyAt<'graph, 'static, 'a>
    where
        'graph: 'a,
    {
        self.laid(lay, segment, None)
    }

    /// The body at `segment`, a composed record laid through `lay` beside `root`.
    fn laid<'held, 'a>(
        &self,
        lay: impl Lays<'a>,
        segment: Segment,
        root: Option<ComposedRoot<'held>>,
    ) -> BodyAt<'graph, 'held, 'a>
    where
        'graph: 'a,
    {
        match self.composed {
            None => BodyAt::Written(&self.load.laid(segment).body),
            Some((_, composition)) => {
                BodyAt::Composed(composition.facts().0.laid(segment).body.laid(lay, root))
            }
        }
    }

    /// `part`, of the body at `segment`, where the load holds it, which outlives every region;
    /// `None` for composed code, which a value born of it may outlive.
    pub fn part(
        &self,
        segment: Segment,
        part: &ExpressionPart<'_>,
    ) -> Option<&'graph ExpressionPart<'graph>> {
        if self.composed.is_some() {
            return None;
        }
        let shape = self.load.shapes.shape_at(segment);
        match shape.located(shape.site(part)) {
            Located::Part(part) => Some(part),
            Located::Node(_) => unreachable!("a part's site locates a part"),
        }
    }

    /// The syntax of the quote at `site` of the body at `segment`, as a code value born of it
    /// holds it.
    pub fn quote(&self, segment: Segment, site: Site) -> CodeSyntax<'graph, 'root>
    where
        'graph: 'root,
    {
        let Some((root, composition)) = self.composed else {
            let shape = self.load.shapes.shape_at(segment);
            return match shape.located(site) {
                Located::Part(ExpressionPart::QuotedExpression(body)) => CodeSyntax::Written {
                    body,
                    segment: (shape.nested(site))
                        .expect("a quote's site holds its code shape")
                        .segment(),
                },
                _ => unreachable!("a quote's site locates its quoted expression"),
            };
        };
        let shapes = composition.facts().shapes();
        let nested = (shapes.shape_at(segment).nested(site))
            .expect("a quote's site holds its code shape")
            .segment();
        CodeSyntax::Composed {
            body: composition.quote_in(root.syntax, nested),
            root,
            segment: Some(nested),
        }
    }
}

/// A body as a value born of it holds it: the base whose shape table holds it, by digest, its
/// segment there, its code digest, and which of its closure slots its content composes — its
/// builder captures but those reading the program's top level, then the type captures its contract
/// names. A value's identity, digest and equality read those alone. Beside them, what a caller
/// needs of a body it did not shape: the root of the composed code it lies in, at `'root`, which
/// finds or rebuilds its composition. One pointer, so a value holds it in place of its shape's.
#[derive(Clone, Copy)]
pub struct BodyRef<'root, 'a> {
    pub base: ContentDigest,
    pub segment: Segment,
    pub code: ContentDigest,
    pub composes: &'a [bool],
    /// `None` for written code.
    pub root: Option<ComposedRoot<'root>>,
}

/// The root of code composed at run time: its syntax, laid where the code value holding it lives,
/// its composition's digest, which finds the composition, and the parameters the composition
/// binds — a callable's, in its shape's channel order, where the code is the untraced body of one,
/// and none for code an `EVAL` or a `BUILD` runs, whose digest is its syntax digest.
#[derive(Clone, Copy, Debug)]
pub struct ComposedRoot<'a> {
    pub syntax: &'a KExpression<'a>,
    pub digest: ContentDigest,
    pub parameters: &'a [BinderSymbol],
}

impl ComposedRoot<'_> {
    /// This root with its syntax and parameters copied through `writer`.
    pub fn copied<'to>(self, writer: Writer<'to, '_>) -> ComposedRoot<'to> {
        ComposedRoot {
            syntax: copied(writer, self.syntax),
            digest: self.digest,
            parameters: collect(writer, self.parameters.iter().copied()),
        }
    }

    /// What laying a copy of this root writes: its syntax and its parameters.
    pub fn weight(&self) -> Weight {
        Weight::syntax(self.syntax).plus(Weight::run::<BinderSymbol>(self.parameters.len()))
    }
}

impl BodyRef<'_, '_> {
    /// Whether the closure slot `at` composes: one past the slots recorded does.
    pub fn composed(&self, at: usize) -> bool {
        self.composes.get(at).copied().unwrap_or(true)
    }

    /// This record copied through `lay` beside `root`.
    pub fn laid<'root, 'to>(
        &self,
        lay: impl Lays<'to>,
        root: Option<ComposedRoot<'root>>,
    ) -> &'to BodyRef<'root, 'to> {
        resident(
            lay,
            BodyRef {
                base: self.base,
                segment: self.segment,
                code: self.code,
                composes: collect(lay, self.composes.iter().copied()),
                root,
            },
        )
    }

    /// What laying a copy of this record costs a rebuild, its root's syntax included: nothing for
    /// written code, whose record the load holds.
    pub fn weight(&self) -> Weight {
        match self.root {
            None => Weight::ZERO,
            Some(root) => Weight::flat::<BodyRef<'_, '_>>()
                .plus(Weight::run::<bool>(self.composes.len()))
                .plus(root.weight()),
        }
    }
}

/// Where a birth in `born_in` reads `binder`: in its slot or capture, a hop per block around
/// `born_in`, or the capture the callable or `MODULE` body holding `born_in` keeps of it
/// ([`Folds::binders`]).
fn binder_coordinate(
    shapes: &Shapes<'_>,
    folds: &[Folds<'_>],
    born_in: Segment,
    binder: Unknown,
) -> Option<CaptureRead> {
    let (mut at, mut hops) = (born_in, 0);
    loop {
        if at == binder.segment {
            let target = match binder.at {
                Address::Slot(slot) => Target::Local(slot),
                Address::Capture(capture) => Target::Capture(capture),
                Address::Held(site) => {
                    let held = folds.get(at.index() as usize)?.held;
                    let index = held.binary_search(&site).ok()? as u32;
                    return Some(CaptureRead::Held { hops, index });
                }
            };
            return Some(CaptureRead::At(Coordinate::Activation { hops, target }));
        }
        let shape = shapes.shape_at(at);
        match shape.kind() {
            super::ShapeKind::Block => {
                at = shapes.parent(at)?.0;
                hops += 1;
            }
            super::ShapeKind::Callable | super::ShapeKind::Module => {
                let index = folds
                    .get(at.index() as usize)?
                    .binders
                    .binary_search(&binder)
                    .ok()?;
                let past = shape.captures().len() + shapes.layout(at).len();
                let target = Target::Capture(CaptureSlot((past + index) as u32));
                return Some(CaptureRead::At(Coordinate::Activation { hops, target }));
            }
            super::ShapeKind::Code | super::ShapeKind::Program => return None,
        }
    }
}

/// Which closure slots of `shape` its content composes: each builder capture reading no top-level
/// slot, then each type capture as `contract` says.
fn composes<'graph>(
    lay: impl Lays<'graph>,
    shape: &BodyShape<'_>,
    contract: impl Fn(usize) -> bool,
    types: usize,
) -> &'graph [bool] {
    let builder = shape.captures().len();
    lay.fill(builder + types, |at| match at.checked_sub(builder) {
        None => shape.composes(super::shape::CaptureSlot(at as u32)),
        Some(index) => contract(index),
    })
}

/// Where a birth reads a capture past its builder captures: at a coordinate, or, for a binder
/// that is a part held in place, at held slot `index` of the activation `hops` enclosing block
/// activations out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaptureRead {
    At(Coordinate),
    Held { hops: u32, index: u32 },
}

/// One body as the load lays it.
#[derive(Clone, Copy)]
pub(crate) struct Laid<'graph> {
    body: BodyRef<'graph, 'graph>,
    /// Where a birth reads each capture past the builder captures: the type captures, then the
    /// binders ([`Folds::binders`]).
    captures: &'graph [CaptureRead],
    folds: Option<Folds<'graph>>,
}

/// A fact a unit's record holds that the shape table cannot place, by what it is found by. A slot's,
/// a statement's and a birth's unit is the shape's to name: [`Shapes::slot_unit`],
/// [`Shapes::running`] and [`Shapes::birth_unit`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Held {
    Site(Segment, Site),
    /// A path entry, by its chain root and name.
    Path(Segment, PathName),
}

/// What holds a module of a tree in a body: a module binder, by its slot, a view written in
/// place, by the site of the `:|` building it, or a part held in place, by its site. Binders
/// order first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Holder {
    Slot(Slot),
    Site(Site),
    Held(Site),
}

impl<'graph> LoadFacts<'graph> {
    /// The facts of a load that built no shape: what a graph that runs no program reads.
    pub const EMPTY: Self = FactsOver(LoadRecords {
        shapes: &Shapes::EMPTY,
        layer: &BuildLayer::EMPTY,
        bodies: &[],
    });

    /// The bytes of this load's records index.
    #[cfg(test)]
    pub fn index_bytes(&self) -> usize {
        std::mem::size_of_val(self.0.layer.index.entries())
    }

    /// The bytes of the facts this load's records index keys: each own record's paths, and each
    /// body's site-keyed and through tables in it.
    #[cfg(test)]
    pub fn keyed_fact_bytes(&self) -> usize {
        use std::mem::size_of_val;
        let typed = |typed: &Typed<'_>| {
            size_of_val(typed.expressions.entries())
                + size_of_val(typed.parts.entries())
                + size_of_val(typed.settled.entries())
                + size_of_val(typed.instances.entries())
                + size_of_val(typed.uses.entries())
                + size_of_val(typed.selections.entries())
                + size_of_val(typed.by_name.entries())
                + size_of_val(typed.routes.entries())
                + size_of_val(typed.arms.entries())
                + size_of_val(typed.retypes.entries())
                + size_of_val(typed.trees.entries())
                + size_of_val(typed.picks.entries())
                + size_of_val(typed.minting.entries())
        };
        (self.own_units())
            .map(|(_, (facts, _))| {
                size_of_val(facts.paths.entries())
                    + facts.typed.iter().map(|(_, at)| typed(at)).sum::<usize>()
            })
            .sum()
    }

    /// The facts of a load whose walk ran nothing over `shapes`: every fact unknown, and every
    /// capture composing.
    #[cfg(test)]
    pub fn unwalked(lay: impl Lays<'graph>, shapes: &'graph Shapes<'graph>) -> Self {
        FactsOver(LoadRecords {
            shapes,
            layer: &BuildLayer::EMPTY,
            bodies: &[],
        })
        .folded(lay, &[])
    }

    /// One walk's records over `shapes`, laid through `lay`: `units` and `closings` sorted by id,
    /// not yet folded.
    pub fn of_records(
        lay: impl Lays<'graph>,
        scratch: BumpAllocator<'_>,
        shapes: &'graph Shapes<'graph>,
        derived: Derived<'graph>,
        units: &[(UnitId, UnitRecord<'graph>)],
        closings: &[(Segment, (&'graph Closing<'graph>, Frontier<'graph>))],
    ) -> Self {
        let layer = BuildLayer::laid(lay, scratch, shapes, units, closings).deriving(derived);
        FactsOver(LoadRecords {
            shapes,
            layer: resident(lay, layer),
            bodies: &[],
        })
    }

    /// These records over `layer` in place of their base layer: a composition's, once its terms
    /// and refusal are known.
    pub fn rebased(&self, layer: &'graph BuildLayer<'graph>) -> Self {
        FactsOver(LoadRecords { layer, ..self.0 })
    }

    /// The load's base layer.
    pub fn base_layer(&self) -> &'graph BuildLayer<'graph> {
        self.0.layer
    }

    /// These records beside `folds`, by segment: every capture composing where it is empty.
    pub fn folded(&self, lay: impl Lays<'graph>, folds: &[Folds<'graph>]) -> Self {
        let shapes = self.0.shapes;
        debug_assert!(folds.is_empty() || folds.len() == shapes.bodies().len());
        let bodies = shapes.bodies();
        let base = shapes.root().code_digest();
        let all = folds;
        let bodies = lay.fill(bodies.len(), |at| {
            let shape = bodies[at];
            let layout = shapes.layout(shape.segment());
            let folds = folds.get(at).copied();
            let binders = folds.map_or(&[][..], |folds| folds.binders);
            let contract = |index: usize| {
                index < layout.len() && folds.is_none_or(|folds| folds.contract[index])
            };
            let born_in = shapes.parent(shape.segment()).map(|(parent, _)| parent);
            let binder_at = |binder: &Unknown| {
                born_in
                    .and_then(|born_in| binder_coordinate(shapes, all, born_in, *binder))
                    .expect("a body's binder lies around it, read through each body between")
            };
            Laid {
                body: BodyRef {
                    base,
                    segment: shape.segment(),
                    code: shape.code_digest(),
                    composes: composes(lay, shape, contract, layout.len() + binders.len()),
                    root: None,
                },
                captures: lay.fill(layout.len() + binders.len(), |at| {
                    match at.checked_sub(layout.len()) {
                        None => CaptureRead::At(layout[at].1),
                        Some(index) => binder_at(&binders[index]),
                    }
                }),
                folds,
            }
        });
        FactsOver(LoadRecords { bodies, ..self.0 })
    }

    /// The record of `unit` beside its frontier.
    pub fn unit(&self, unit: UnitId) -> Option<(&'graph UnitFacts<'graph>, Frontier<'graph>)> {
        self.0.unit(unit)
    }

    /// The closing record of the owning body `body`, beside its frontier.
    pub fn closing(&self, body: Segment) -> Option<(&'graph Closing<'graph>, Frontier<'graph>)> {
        self.0.closing(body)
    }

    /// Hand `each` every type these records name.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        self.0.layer.each_type(each);
    }

    /// The units this load marked unreached, sorted.
    #[cfg(test)]
    pub fn unreached_units(&self) -> impl Iterator<Item = UnitId> + 'graph {
        self.0.layer.unreached_units()
    }

    /// The units this load ran itself, each beside its record and frontier, sorted.
    pub fn own_units(
        &self,
    ) -> impl Iterator<Item = (UnitId, (&'graph UnitFacts<'graph>, Frontier<'graph>))> + 'graph
    {
        (self.0.layer.units.entries().iter())
            .filter_map(|(unit, record)| Some((*unit, record.typed()?)))
    }

    /// The base's own closing records, by owning body.
    pub fn own_closings(
        &self,
    ) -> impl Iterator<Item = (Segment, (&'graph Closing<'graph>, Frontier<'graph>))> + 'graph {
        self.0.layer.closings.entries().iter().copied()
    }

    /// Each site question the base's own records store: its unit records', then its closing
    /// records'.
    #[cfg(test)]
    pub fn own_questions(&self) -> impl Iterator<Item = &'graph SiteQuestion<'graph>> + 'graph {
        let units = (self.own_units()).flat_map(|(_, (facts, _))| facts.questions.iter());
        let closings = (self.own_closings()).flat_map(|(_, (closing, _))| closing.questions.iter());
        units.chain(closings)
    }
}

#[expect(
    private_bounds,
    reason = "a source of records is the crate's own; outside it only the two aliases are named"
)]
impl<'a, S: RecordSource<'a>> FactsOver<S> {
    /// `fact`, read off these records, with every type it names resolving in `types`, the reading
    /// frame's chain: each the chain lacks is minted from its term ([`RecordSource::term`]). A frame
    /// mints only what it reads, once per tier the chain interns in. Over the load and the
    /// under the root, every type a fact names lies in the root or the agent tier
    /// already. Every reader of a fact naming a type reads through it, so none can skip it.
    fn minted<T: Named>(&self, types: &Chain<'_, '_, '_, '_>, fact: T) -> T {
        if !self.0.composed() {
            return fact;
        }
        fact.each_named(&mut |kt| {
            if types.resolves(kt) {
                return;
            }
            #[cfg(test)]
            TERM_PROBES.with(|probes| probes.set(probes.get() + 1));
            match self.0.term(kt.digest()) {
                Some(term) => types.mint(term),
                None => debug_assert!(false, "a type a fact names has a term"),
            }
        });
        fact
    }

    /// `fact`, read off these records, closed and minted ([`minted`](Self::minted)): each type it
    /// names written over a name the reading frame's slots bind takes the type the binding gives
    /// it, written in by the program's fact domain ([`Rebinds`](super::Rebinds)). Then every type
    /// it names resolves. A fact naming no such type is handed back as read.
    fn resolved<T: Named + Close>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        segment: Segment,
        fact: T,
    ) -> T {
        let fact = self.minted(types, fact);
        let fact = match self.0.rebinding() {
            Some((rebinds, binding)) => {
                let scratch = crate::memory::Bump::new();
                fact.closed(
                    &mut Rebinding::<'_, '_, '_, '_, '_, '_, BumpAllocator<'_>> {
                        rebinds,
                        binding,
                        segment,
                        types,
                        scratch: &scratch,
                        lay: None,
                    },
                )
            }
            None => fact,
        };
        self.minted(types, fact)
    }

    /// [`resolved`](Self::resolved) for a fact holding a slice of types, which closing copies into
    /// `scratch` where some element changes.
    fn resolved_laying<'l, T: Named + CloseLaying<'l>>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        segment: Segment,
        fact: T,
    ) -> T {
        let fact = self.minted(types, fact);
        let fact = match self.0.rebinding() {
            Some((rebinds, binding)) => fact.closed_laying(&mut Rebinding {
                rebinds,
                binding,
                segment,
                types,
                scratch,
                lay: Some(scratch),
            }),
            None => fact,
        };
        self.minted(types, fact)
    }

    /// `fact`, a fact of `shape` read at `site` where the cell runs, as a reader handed it back
    /// with its types resolving in `types`: closed, refused at every run, or left open to the run's
    /// own door. A rigid fact never reaches the run — every variable the
    /// load left open is bound in a layer the cell reads — save one over a path the load read off a
    /// signature lacking its member, which every run refuses; any other is a bug, refused as the
    /// load refuses a name with no binding.
    pub fn at_run<C, R>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        site: Site,
        fact: Static<'a, C, R>,
    ) -> AtRun<C> {
        match fact {
            Static::Closed(value) => AtRun::Closed(value),
            Static::Unknown => AtRun::Open,
            Static::Rigid { variables, .. } => {
                AtRun::Refused(self.unbound(types, shape, site, variables))
            }
        }
    }

    /// The type the type expression at `site` of `shape` denotes where the cell runs, or why every
    /// run refuses it. The load, or a layer the cell reads, fixed it: one left open is a bug.
    pub fn denoted(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> Result<KType, Elaboration> {
        match self.at_run(
            types,
            shape,
            site,
            self.typed_expression(types, shape, site),
        ) {
            AtRun::Closed(closed) => Ok(closed),
            AtRun::Refused(refused) => Err(refused),
            AtRun::Open => {
                debug_assert!(
                    false,
                    "the type at {site:?} of body {:?} is open where it runs",
                    shape.segment()
                );
                Err(Elaboration::OpenAtRun { site })
            }
        }
    }

    /// Why a rigid fact of `shape` over `variables`, read at `site`, refuses.
    fn unbound(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        site: Site,
        variables: &[Variable],
    ) -> Elaboration {
        let shapes = self.shapes();
        let path = |root: Segment, name| self.path(types, shapes.shape_at(root), name);
        for variable in variables {
            if let Variable::Path { root, entry, .. } = *variable
                && let Some((entry, resolution)) = path(root, entry)
                && let Some(owner) = resolution.lacks
            {
                return Elaboration::NoSuchMember {
                    owner,
                    name: entry.label.symbol(),
                    site,
                };
            }
        }
        #[cfg(test)]
        rigid_at_run();
        debug_assert!(
            false,
            "a rigid fact of body {:?} at {site:?} reached the run",
            shape.segment()
        );
        let name = variables.iter().find_map(|variable| match *variable {
            Variable::Type { at, .. } => match shapes.follow(shape.segment(), at)? {
                Lands::Binder { at, slot } => Some(shapes.shape_at(at).slot_name(slot).symbol()),
                _ => None,
            },
            Variable::Path { root, entry, .. } => Some(path(root, entry)?.0.label.symbol()),
        });
        match name {
            Some(name) => Elaboration::Unbound { name, site },
            None => Elaboration::OpenAtRun { site },
        }
    }

    /// The shape table these are facts of.
    pub fn shapes(&self) -> &'a Shapes<'a> {
        self.0.shapes()
    }

    /// The term of the derived name `id`, which the innermost layer that laid it holds.
    pub fn term_of(&self, id: crate::type_lattice::DerivedId) -> Option<DerivedTerm<'a>> {
        self.0.derived_name(id).map(|name| name.term)
    }

    /// The derived name `id`, which the innermost layer that laid it holds.
    pub fn derived_name(&self, id: crate::type_lattice::DerivedId) -> Option<DerivedName<'a>> {
        self.0.derived_name(id)
    }

    /// What any layer binds `unknown` to.
    pub fn bound(&self, unknown: Unknown) -> Option<Bound<'a>> {
        self.0.bound(unknown)
    }

    /// Hand `each` every unknown any layer binds, beside what it binds it to, the innermost
    /// layer's first.
    pub fn each_bound(&self, each: &mut dyn FnMut(Unknown, Bound<'a>)) {
        self.0.each_bound(each);
    }

    /// The closed tree whose identity is `digest`, among those these records computed.
    pub fn tree_of(&self, digest: ContentDigest) -> Option<&'a Tree<'a>> {
        self.0.tree_of(digest)
    }

    /// `segment`'s facts in the record holding `held`.
    fn typed(&self, segment: Segment, held: Held) -> Option<&'a Typed<'a>> {
        let (_, facts) = self.0.holding(held)?;
        facts.typed.get(segment).map(|(_, typed)| typed)
    }

    /// `shape` as this load's table holds it, found by its segment: so a reader's shape may be
    /// borrowed for any lifetime.
    fn at(&self, shape: &BodyShape<'_>) -> &'a BodyShape<'a> {
        let at = self.0.shapes().shape_at(shape.segment());
        debug_assert!(std::ptr::addr_eq(at, shape), "a shape of this load");
        at
    }

    /// The entry at `key` of the `table` of the facts the unit holding `held` fixed for `shape`.
    fn fact<K: Ord + Copy + 'a, V: Copy + 'a>(
        &self,
        shape: &BodyShape<'_>,
        held: Held,
        table: impl FnOnce(&Typed<'a>) -> Keyed<'a, (K, V)>,
        key: K,
    ) -> Option<V> {
        let typed = self.typed(shape.segment(), held)?;
        table(typed).get(key).map(|(_, value)| *value)
    }

    /// What the instance the reading frame holds answers the stored question of `rule` at `site`
    /// of `shape`: `None` where it holds none owning it, and the load's record stands.
    pub fn site_answer(
        &self,
        shape: &BodyShape<'_>,
        site: Site,
        rule: SiteRule,
    ) -> Option<SiteAnswer> {
        self.0.site_answer(shape.segment(), site, rule)
    }

    /// The entry at `site` of the site-keyed `table` of `shape`'s facts.
    fn at_site<V: Copy + 'a>(
        &self,
        shape: &BodyShape<'_>,
        site: Site,
        table: impl FnOnce(&Typed<'a>) -> Keyed<'a, (Site, V)>,
    ) -> Option<V> {
        self.fact(shape, Held::Site(shape.segment(), site), table, site)
    }

    /// `shape`'s facts in the newest record of `unit`.
    fn typed_in(&self, shape: &BodyShape<'_>, unit: UnitId) -> Option<&'a Typed<'a>> {
        let (facts, _) = self.0.unit(unit)?;
        facts.typed.get(shape.segment()).map(|(_, typed)| typed)
    }

    /// The entry at `slot` of the slot-keyed `table` of `shape`'s facts.
    fn at_slot<V: Copy + 'a>(
        &self,
        shape: &BodyShape<'_>,
        slot: Slot,
        table: impl FnOnce(&Typed<'a>) -> Keyed<'a, (Slot, V)>,
    ) -> Option<V> {
        let typed = self.typed_in(shape, self.0.shapes().slot_unit(shape.segment(), slot))?;
        table(typed).get(slot).map(|(_, value)| *value)
    }

    /// The birth facts of `body`, a callable or a quote's code, in its birth unit's newest record.
    fn born(&self, body: Segment) -> Option<&'a Born<'a>> {
        let (facts, _) = self.0.unit(self.0.shapes().birth_unit(body)?)?;
        facts.born.get(body).map(|(_, born)| born)
    }

    /// Whether the site-listing `table` of `shape`'s facts lists `site`.
    fn lists_site(
        &self,
        shape: &BodyShape<'_>,
        site: Site,
        table: impl FnOnce(&Typed<'a>) -> Keyed<'a, Site>,
    ) -> bool {
        (self.typed(shape.segment(), Held::Site(shape.segment(), site)))
            .is_some_and(|typed| table(typed).get(site).is_some())
    }

    /// What the load fixed for the type expression at `site` of `shape`: `Unknown` where it records
    /// none there.
    pub fn typed_expression(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> StaticType<'a> {
        let typed = self.at_site(shape, site, |typed| typed.expressions);
        self.resolved(types, shape.segment(), typed.unwrap_or(Static::Unknown))
    }

    /// What the load fixed for the type binder at `slot` of `shape`.
    pub fn declared_type(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        slot: Slot,
    ) -> StaticType<'a> {
        let declared = self.at_slot(shape, slot, |typed| typed.declared);
        self.resolved(types, shape.segment(), declared.unwrap_or(Static::Unknown))
    }

    /// What the load fixed for the registration at `slot` of `shape`.
    pub fn registered_type(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        slot: Slot,
    ) -> StaticRegistered<'a> {
        let registered = self.at_slot(shape, slot, |typed| typed.registered);
        self.resolved(
            types,
            shape.segment(),
            registered.unwrap_or(Static::Unknown),
        )
    }

    /// Whether the callable body `body` is born with `name` among its `FOR ALL` names: where a
    /// call's solution, not an argument, binds it.
    pub fn quantifies(&self, body: Segment, name: BinderSymbol) -> bool {
        let BinderSymbol::Type(name) = name else {
            return false;
        };
        let born = self.born(body);
        match born.map(|born| born.callable) {
            Some(Static::Closed(callable)) => callable.quantifier_map.get(name).is_some(),
            Some(Static::Rigid { value, .. }) => value.quantifier_map.get(name).is_some(),
            _ => false,
        }
    }

    /// What the load fixed for the callable body `shape`'s type.
    pub fn callable_type(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
    ) -> StaticCallable<'a> {
        let callable = (self.born(shape.segment())).map_or(Static::Unknown, |born| born.callable);
        self.resolved(types, shape.segment(), callable)
    }

    /// The callable body `shape`'s own `FOR ALL` group as its body reads it, by group order.
    pub fn group_levels(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
    ) -> &'a [Parametric] {
        self.minted(
            types,
            self.born(shape.segment()).map_or(&[], |born| born.levels),
        )
    }

    /// The solution the callable body `shape`'s `FOR ALL` is born instantiated at, where the load
    /// solved one.
    pub fn born_instance<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
    ) -> Option<StaticInstance<'l>>
    where
        'a: 'l,
    {
        let instance: Option<StaticInstance<'l>> = self.born(shape.segment())?.instance;
        self.resolved_laying(types, scratch, shape.segment(), instance)
    }

    /// Each lexical variable `shape` declares, by level, beside where its activation holds it.
    pub fn homes(&self, shape: &BodyShape<'_>) -> &'a [(usize, Target)] {
        self.0.shapes().homes(shape.segment())
    }

    /// Each capture of `shape` past its builder captures, as where its birth reads it: its
    /// type captures, then its binders ([`Folds::binders`]).
    pub fn type_captures(&self, shape: &BodyShape<'_>) -> &'a [CaptureRead] {
        self.0.laid(shape.segment()).captures
    }

    /// The binder of a body around `shape` its capture `index` past its type captures holds
    /// ([`Folds::binders`]).
    pub fn capture_binder(&self, shape: &BodyShape<'_>, index: usize) -> Option<Unknown> {
        self.0.folds(shape.segment())?.binders.get(index).copied()
    }

    /// How many closure slots a birth of `shape` fills: its builder captures, its type captures,
    /// then its binders.
    pub fn capture_count(&self, shape: &BodyShape<'_>) -> usize {
        shape.captures().len() + self.type_captures(shape).len()
    }

    /// Which of `shape`'s type captures its content composes, parallel to
    /// [`type_captures`](Self::type_captures).
    pub fn contract(&self, shape: &BodyShape<'_>) -> &'a [bool] {
        &self.0.laid(shape.segment()).body.composes[shape.captures().len()..]
    }

    /// The path `shape` spells at `site`, where it spells one its chain numbers: the segment of
    /// the body rooting its chain, beside its level there. A text fact, read off the shape table.
    pub fn path_read(&self, shape: &BodyShape<'_>, site: Site) -> Option<(Segment, usize)> {
        let index = shape.spelled_at(site)?;
        let level = self.0.shapes().path_level(shape.segment(), index).ok()?;
        Some((self.0.shapes().chain_root(shape.segment()), level))
    }

    /// The path entry `name` of the chain the body `root` roots, beside what the load resolved it
    /// to.
    pub fn path(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        root: &BodyShape<'_>,
        name: PathName,
    ) -> Option<(PathEntry<'a>, Resolution<'a>)> {
        let (_, facts) = self.0.holding(Held::Path(root.segment(), name))?;
        let path = facts
            .paths
            .get((root.segment(), name))
            .map(|(_, path)| *path);
        self.resolved(types, root.segment(), path)
    }

    /// The first registration of the quote's code `shape` that overlaps a builtin overload,
    /// reported when an `EVAL` runs it.
    pub fn overlap(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
    ) -> Option<&'a ShapeError<'a>> {
        let overlap = (self.0.closing(shape.segment())).and_then(|(closing, _)| closing.overlap);
        // An overlap names a builtin overload's closed type: nothing to close.
        self.minted(types, overlap)
    }

    /// Why the code shape `shape`'s code cannot be built or does not type, reported when an `EVAL`
    /// runs it, worded as a load with its bindings declared words it: closed through the chain's
    /// tables, as a re-run under them reads it, then each capture of the code through `captured`,
    /// the closed type the `EVAL`'s code binds it to, where it binds one. Laid in `scratch` where
    /// some type closes.
    pub fn typing_refusal<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        captured: &mut dyn FnMut(CaptureSlot) -> Option<KType>,
    ) -> Option<&'l ShapeError<'l>>
    where
        'a: 'l,
    {
        let refusal: Option<&'l ShapeError<'l>> = self.refused(shape);
        let refusal = self.resolved_laying(types, scratch, shape.segment(), refusal)?;
        let (shapes, segment) = (self.0.shapes(), shape.segment());
        let captured = std::cell::RefCell::new(captured);
        Some(refusal.closed_laying(&mut Capturing {
            types,
            lay: scratch,
            bound: &|level| match shapes.home(segment, level)? {
                Target::Capture(capture) => (captured.borrow_mut())(capture),
                Target::Local(_) => None,
            },
        }))
    }

    /// `refusal`, an arm of a question a bind site's instance refuses in the body `segment`,
    /// worded as a load with the frame's binding declared words it: closed by the binding, laid in
    /// `scratch` where some type closes.
    pub fn bound_refusal<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        segment: Segment,
        refusal: &'l ShapeError<'l>,
    ) -> &'l ShapeError<'l>
    where
        'a: 'l,
    {
        self.resolved_laying(types, scratch, segment, refusal)
    }

    /// Whether the code shape `shape`'s code cannot be built or does not type: a binding closes a
    /// refusal's wording, never whether there is one.
    pub fn refuses(&self, shape: &BodyShape<'_>) -> bool {
        self.refused(shape).is_some()
    }

    /// The refusal of the code shape `shape`, as laid.
    fn refused(&self, shape: &BodyShape<'_>) -> Option<&'a ShapeError<'a>> {
        // A code shape the builder could not build holds its refusal itself.
        (self.0.closing(shape.segment()))
            .and_then(|(closing, _)| closing.refusal)
            .or_else(|| self.at(shape).refusal())
    }

    /// The static type of the part at `site` of `shape`, where the load typed one: the load's
    /// record, closed under a frame's binding.
    pub fn value_type(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> Option<Interval> {
        let typed = self.at_site(shape, site, |typed| typed.parts);
        self.resolved(types, shape.segment(), typed)
    }

    /// The static type of statement `index` of `shape`'s body.
    pub fn statement_type(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        index: usize,
    ) -> Option<Interval> {
        let index = index as u32;
        let typed = self.typed_in(shape, self.0.shapes().running(shape.segment(), index))?;
        self.resolved(
            types,
            shape.segment(),
            typed.statements.get(index).map(|(_, typed)| *typed),
        )
    }

    /// The static type of the binder at `slot` of `shape`.
    pub fn binder_type(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        slot: Slot,
    ) -> Option<Holding<'a>> {
        self.resolved(
            types,
            shape.segment(),
            self.at_slot(shape, slot, |typed| typed.binders),
        )
    }

    /// What the load fixed about the keyworded use whose node sits at `site` of `shape`.
    pub fn use_statics<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> UseStatics<'l>
    where
        'a: 'l,
    {
        let read = self.use_read(types, scratch, shape, site);
        self.use_statics_by(types, scratch, shape, site, read)
    }

    /// What the instance the reading frame holds answers at the keyworded use at `site` of
    /// `shape`: what a call reads once and hands each of [`use_statics_by`](Self::use_statics_by),
    /// [`selection_by`](Self::selection_by) and [`route_by`](Self::route_by).
    pub fn use_read<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> UseRead<'a, 'l>
    where
        'a: 'l,
    {
        let (segment, held) = (shape.segment(), Held::Site(shape.segment(), site));
        let facts = self.0.holding(held).map(|(_, facts)| facts);
        let question =
            facts.and_then(|facts| facts.questions.get((segment, site, SiteRule::Selection)));
        UseRead {
            site,
            typed: facts
                .and_then(|facts| facts.typed.get(segment))
                .map(|(_, typed)| typed),
            answered: question
                .and_then(|question| self.answered_use(types, scratch, shape, site, question)),
        }
    }

    /// [`use_statics`](Self::use_statics) over the instance's answer `read`.
    pub fn use_statics_by<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
        read: UseRead<'a, 'l>,
    ) -> UseStatics<'l>
    where
        'a: 'l,
    {
        let record: UseStatics<'l> = read
            .record(|typed| typed.uses)
            .unwrap_or(UseStatics::UNFIXED);
        let answered = self.answered_narrowing(types, scratch, shape, site, read);
        // The load's record, its contributions closed under the frame's binding, as the points it
        // solves by are; its narrowing the instance's where the instance answers the selection.
        let statics = UseStatics {
            narrowing: answered.unwrap_or(record.narrowing),
            ..record
        };
        let statics = self.resolved_laying(types, scratch, shape.segment(), statics);
        // A contribution the binding closes to `Any` is the argument's carried type, as the load
        // reads an upper end of `Any`: the points the load solved from it do not hold, so the call
        // admits and solves each candidate the load solved, as the load leaves one with no point.
        let narrowing = match statics.contributions.iter().any(closed_to_any) {
            true => solved_at_call(scratch, statics.narrowing),
            false => statics.narrowing,
        };
        UseStatics {
            narrowing,
            contributions: carried_at_any(scratch, statics.contributions),
            ..statics
        }
    }

    /// The candidates of the keyworded use at `site` of `shape` as the instance the reading frame
    /// holds narrows them, laid in `scratch`: the one it selects, or those it keeps, each beside its
    /// verdict and an *always* one's points. Where the instance ranks them, the load's table does
    /// where it keeps what the load kept, else [`ranked_table`](Self::ranked_table); the rest rank
    /// at the call.
    fn answered_narrowing<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
        read: UseRead<'a, 'l>,
    ) -> Option<Narrowing<'l>>
    where
        'a: 'l,
    {
        let (answer, picked) = read.answered?;
        let listed = shape.candidates(site)?.candidates;
        let listed_at = |picked: &Picked<'_, '_>| listed[usize::from(picked.selectable.position)];
        let SiteAnswer::Kept { ranked, always, .. } = answer else {
            return match listed_at(&picked[0]) {
                Candidate::One(coordinate) => Some(Narrowing::Selected(coordinate)),
                Candidate::Spread(_) => None,
            };
        };
        if picked.len() == listed.len() && !ranked && always == 0 {
            return Some(Narrowing::Full);
        }
        let entries = scratch.fill(picked.len(), |at| {
            (listed_at(&picked[at]), picked[at].verdict)
        });
        let points = scratch.fill(picked.len(), |at| picked[at].points.unwrap_or(Points::NONE));
        let base = read.record(|typed| typed.uses);
        let elimination = match base.map(|base| base.narrowing) {
            _ if !ranked => Elimination::Open,
            Some(Narrowing::Kept(Kept {
                entries: kept,
                elimination: Elimination::Table(table),
                ..
            })) if (kept.iter().map(|(candidate, _)| candidate))
                .eq(entries.iter().map(|(candidate, _)| candidate)) =>
            {
                Elimination::Table(*table)
            }
            // A pair the load left open the binding ranks: its table, as a fresh load lays it.
            _ => (self.ranked_table(types, scratch, shape.segment(), picked))
                .map_or(Elimination::Open, Elimination::Table),
        };
        let kept = Kept {
            entries,
            points,
            elimination,
        };
        Some(Narrowing::Kept(resident(scratch, kept)))
    }

    /// The shape `shaped`, its type and its bounds over names closed under the reading frame's
    /// binding, rebuilt where every bound closes.
    fn resolved_shape<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        segment: Segment,
        shaped: Shaped<'_>,
    ) -> Shaped<'l> {
        let bounds = shaped.rebounds.0;
        let resolved = Shaped {
            ty: self.resolved(types, segment, shaped.ty),
            rebounds: Rebounds(scratch.alloc_slice_fill_iter(
                (bounds.iter()).map(|(at, bound)| (*at, self.resolved(types, segment, *bound))),
            )),
        };
        resolved.rebuilt(types, scratch)
    }

    /// The table ranking `picked`, the candidates an instance keeps and ranks, laid in `scratch`:
    /// each pair's answer the closed door's to the selection's own ask, its shapes closed under the
    /// reading frame's binding. `None` where a pair's shapes stay open.
    fn ranked_table<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        segment: Segment,
        picked: &[Picked<'a, 'l>],
    ) -> Option<super::typed::Table<'l>> {
        let question = picked.first()?.question;
        let [Payload::Selection(layout), ..] = question.arms else {
            return None;
        };
        let mut open = false;
        let mut outranks = |class: u16, a: usize, b: usize| {
            let asked = (picked[a].selectable.rank).zip(picked[b].selectable.rank);
            let answer = asked.and_then(|(a, b)| {
                let ask = question.asks[usize::from(layout.outranks(class, a, b))].question;
                let Question::Outranks { a, b, class } = ask else {
                    return None;
                };
                let closed = Question::Outranks {
                    a: self.resolved_shape(types, scratch, segment, a),
                    b: self.resolved_shape(types, scratch, segment, b),
                    class,
                };
                super::question::answer(types, scratch, &closed)
            });
            open |= answer.is_none();
            answer == Some(Answer::Bool(true))
        };
        let builtin = |at: usize| picked[at].selectable.builtin;
        let classes = usize::from(layout.classes);
        let table =
            super::typed::Table::laid(scratch, picked.len(), classes, &mut outranks, builtin);
        table.filter(|_| !open)
    }

    /// The candidates the instance the reading frame holds runs at the keyworded use at `site` of
    /// `shape`, laid in `scratch`, beside its answer: the one it selects, or those it keeps in list
    /// order. `None` where it holds none owning the selection, the selection refuses the use, or a
    /// point of one it takes as admitted is not closed.
    fn answered_use<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
        question: &'a SiteQuestion<'a>,
    ) -> Option<(SiteAnswer, &'l [Picked<'a, 'l>])>
    where
        'a: 'l,
    {
        let answer = self.site_answer(shape, site, SiteRule::Selection)?;
        let mut picked = BumpVec::new_in(scratch);
        for (at, verdict) in runs(question, answer)? {
            let selectable = selectable(question, at);
            let points = match verdict {
                Verdict::Always => {
                    let points = answered_points(types, scratch, question, selectable)?;
                    Some(self.resolved_laying(types, scratch, shape.segment(), points))
                }
                _ => None,
            };
            picked.push(Picked {
                selectable,
                verdict,
                points,
                question,
            });
        }
        Some((answer, picked.leak()))
    }

    /// The question of `rule` the load stored at `site` of `shape`.
    fn stored(
        &self,
        shape: &BodyShape<'_>,
        site: Site,
        rule: SiteRule,
    ) -> Option<&'a SiteQuestion<'a>> {
        let (_, facts) = self.0.holding(Held::Site(shape.segment(), site))?;
        facts.questions.get((shape.segment(), site, rule))
    }

    /// The route the load fixed for the tail call whose node sits at `site` of `shape`, where it
    /// knows the callee.
    /// A call by name's whether its callee's return keeps the frame's contract is the instance's
    /// the reading frame holds; a keyworded use's candidates the load leaves open are left to the
    /// call, which checks the contract by the callee it runs.
    pub fn route<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> Option<Route<'l>>
    where
        'a: 'l,
    {
        let read = self.use_read(types, scratch, shape, site);
        self.route_by(scratch, shape, site, read)
    }

    /// [`route`](Self::route) over the instance's answer `read`.
    pub fn route_by<'l>(
        &self,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
        read: UseRead<'a, 'l>,
    ) -> Option<Route<'l>>
    where
        'a: 'l,
    {
        let record = read.record(|typed| typed.routes);
        let answered = (self.answered_route(scratch, shape, site, read))
            .or_else(|| self.answered_use_route(scratch, read));
        answered.or(record)
    }

    /// The route of the tail call at `site` of `shape` over the load's record, a call by name's
    /// kept as the instance the reading frame holds answers: `None` where it holds none owning
    /// the call's route.
    fn answered_route<'l>(
        &self,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
        read: UseRead<'a, 'l>,
    ) -> Option<Route<'l>>
    where
        'a: 'l,
    {
        let base = read.record(|typed| typed.routes)?;
        let answer = self.site_answer(shape, site, SiteRule::TailRoute)?;
        let [route] = base.per else {
            return None;
        };
        let kept = match answer {
            SiteAnswer::Bool(kept) => Some(kept),
            _ => None,
        };
        Some(Route {
            per: scratch.fill(1, |_| CandidateRoute { kept, ..*route }),
        })
    }

    /// The route of the keyworded use `read` reads, in the tail, one per candidate the instance
    /// the reading frame holds narrows it to: whether each keeps the frame's contract where the
    /// load's answers decide it, the asks a binding decides left to the call.
    fn answered_use_route<'l>(
        &self,
        scratch: BumpAllocator<'l>,
        read: UseRead<'a, 'l>,
    ) -> Option<Route<'l>>
    where
        'a: 'l,
    {
        read.record(|typed| typed.routes)?;
        let (_, picked) = read.answered?;
        let per = scratch.fill(picked.len(), |at| {
            let Picked {
                selectable,
                points,
                question,
                ..
            } = picked[at];
            let Some(routing) = selectable.route else {
                return CandidateRoute {
                    kept: None,
                    hop: false,
                };
            };
            // A scheme's return is read at its points, where each is exact.
            let solved = points.is_some_and(|points| {
                !(points.points.iter()).any(|point| matches!(point, Point::Carried(_)))
            });
            let kept = match routing.keeps {
                Some((_, true)) if !solved => None,
                Some(_) => {
                    routing.kept(&mut AskedBy(|at: u16| question.asks[usize::from(at)].rigid))
                }
                None => None,
            };
            CandidateRoute {
                kept,
                hop: routing.hop,
            }
        });
        Some(Route { per })
    }

    /// What the load fixed about the arms of the `MATCH` whose node sits at `site` of `shape`, as
    /// the instance the reading frame holds decides it: the arm a binding chooses, or the arms it
    /// keeps, laid in `scratch`.
    pub fn arms<'l>(
        &self,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> Option<Arms<'l>>
    where
        'a: 'l,
    {
        let record = self.at_site(shape, site, |typed| typed.arms);
        let answered = self.answered_arms(scratch, shape, site);
        answered.or(record)
    }

    /// The arms of the `MATCH` at `site` of `shape` the instance the reading frame holds decides,
    /// over the load's record of them: `None` where it holds none owning its question, or the
    /// question's answer is one no run reaches the `MATCH` under.
    fn answered_arms<'l>(
        &self,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> Option<Arms<'l>>
    where
        'a: 'l,
    {
        let answer = self.site_answer(shape, site, SiteRule::ArmsRun)?;
        let base = self.at_site(shape, site, |typed| typed.arms)?;
        match answer {
            SiteAnswer::Arm(arm) => Some(Arms {
                chosen: Some(arm - 1),
                ..base
            }),
            SiteAnswer::Kept { kept, always, .. } => {
                let mut arms = BumpVec::with_capacity_in(kept.count_ones() as usize, scratch);
                arms.extend((0..64u16).filter(|at| kept & 1 << at != 0).map(|at| {
                    let verdict = match always & 1 << at != 0 {
                        true => Verdict::Always,
                        false => Verdict::Maybe,
                    };
                    (at, verdict)
                }));
                Some(Arms {
                    kept: arms.leak(),
                    ranking: base.ranking,
                    chosen: None,
                })
            }
            _ => None,
        }
    }

    /// What the retype at `site` of `shape` re-stamps at, where the load or the instance the
    /// reading frame holds knows how and the frame's binding closes the type it re-stamps at.
    pub fn retype(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> Option<Retype> {
        let record = self.at_site(shape, site, |typed| typed.retypes);
        let answered = [
            SiteRule::MemberRetype,
            SiteRule::HoldRetype,
            SiteRule::ArmRestamp,
        ]
        .into_iter()
        .find_map(|rule| match self.answered_arm(shape, site, rule)? {
            Payload::Restamp(restamp) => Some(*restamp),
            _ => None,
        });
        let restamp = self.resolved(types, shape.segment(), answered.or(record))?;
        restamp.retype(types)
    }

    /// The payload of the arm the instance the reading frame holds chooses for the stored question
    /// of `rule` at `site` of `shape`: `None` where it holds none owning it, or chooses no arm.
    fn answered_arm(
        &self,
        shape: &BodyShape<'_>,
        site: Site,
        rule: SiteRule,
    ) -> Option<&'a Payload<'a>> {
        let SiteAnswer::Arm(arm) = self.site_answer(shape, site, rule)? else {
            return None;
        };
        self.stored(shape, site, rule)?.arms.get(usize::from(arm))
    }

    /// What a frame of the callable `shape` re-stamps its arguments and its value at, where the
    /// load closed their declared types.
    pub fn retypes<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
    ) -> Retypes<'l>
    where
        'a: 'l,
    {
        let retypes = self.0.closing(shape.segment());
        self.resolved_laying(
            types,
            scratch,
            shape.segment(),
            retypes.map_or(Retypes::NONE, |(closing, _)| closing.retypes),
        )
    }

    /// The class the load fixed for the frame body `shape`.
    pub fn class(&self, shape: &BodyShape<'_>) -> Option<BodyClass> {
        self.0.closing(shape.segment())?.0.class
    }

    /// Whether the frame body `shape` caches.
    pub fn caches(&self, shape: &BodyShape<'_>) -> bool {
        self.caching(shape).is_some_and(|caching| caching.caches)
    }

    /// Where `shape`'s types intern at run.
    pub fn caching(&self, shape: &BodyShape<'_>) -> Option<&'a Caching> {
        self.0.folds(shape.segment())?.caching
    }

    /// Whether the load recorded `site` as one of `shape`'s own minting sites.
    pub fn mints_at(&self, shape: &BodyShape<'_>, site: Site) -> bool {
        self.lists_site(shape, site, |typed| typed.minting)
    }

    /// The content tree of the view the `:|` at `site` of `shape` builds, where its signature is
    /// closed.
    pub fn view_tree(&self, shape: &BodyShape<'_>, site: Site) -> Option<&'a Tree<'a>> {
        self.at_site(shape, site, |typed| typed.trees)
    }

    /// What the view the `:|` at `site` of `shape` builds carries for each keyworded member, where
    /// the load sees all of the body-born module it views: the load records picks only over a
    /// closed tree, the same at every binding; elsewhere the run computes the covers.
    pub fn picks(&self, shape: &BodyShape<'_>, site: Site) -> Option<&'a [Pick<'a>]> {
        self.at_site(shape, site, |typed| typed.picks)
    }

    /// Whether the load settled the ascription at `site` of `shape`, or the instance the reading
    /// frame holds settles it: its value fits its type at every run of the frame.
    pub fn settled(&self, shape: &BodyShape<'_>, site: Site) -> bool {
        let record = self.lists_site(shape, site, |typed| typed.settled);
        let answered = [SiteRule::Hold, SiteRule::View]
            .into_iter()
            .find_map(|rule| self.site_answer(shape, site, rule))
            .map(|answer| answer == SiteAnswer::Bool(true));
        // An instance may leave to the run a check the walk settled, where module substitution
        // cannot close a path through a signature meet; it never settles one the walk did not.
        answered.unwrap_or(record)
    }

    /// What the load fixed where `shape` reads a quantified function at `site`: the load's record,
    /// closed under a frame's binding. An instance the binding finds no instance under refuses
    /// at its bind site. A member of a module the load cannot see is instanced once the binding
    /// closes its module's carried type, as its member, or read as it is where the member is
    /// declared at a type; the member read itself while the binding leaves it open.
    pub fn instance_at<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> Option<InstanceRead<'l>>
    where
        'a: 'l,
    {
        let closed = |read: Option<InstanceRead<'l>>| {
            self.resolved_laying(types, scratch, shape.segment(), read)
        };
        let read = self.at_site(shape, site, |typed| typed.instances)?;
        let read = match read {
            InstanceRead::Member {
                name,
                carried,
                wanted:
                    MemberWanted::OfUse {
                        site: used,
                        argument,
                    },
            } => {
                let wanted = self.member_wanted(types, scratch, shape, used, argument)?;
                InstanceRead::Member {
                    name,
                    carried,
                    wanted: wanted.map_or(MemberWanted::Nothing, MemberWanted::At),
                }
            }
            read => read,
        };
        match closed(Some(read))? {
            InstanceRead::Member {
                name,
                carried,
                wanted,
            } => {
                let open = InstanceRead::Member {
                    name,
                    carried,
                    wanted,
                };
                let Some(carried) = types.concrete(carried) else {
                    return Some(open);
                };
                let wanted = match wanted.at().map(|wanted| types.concrete(wanted)) {
                    Some(None) => return Some(open),
                    Some(Some(wanted)) => Some(wanted),
                    None => None,
                };
                match super::application::member_instance_closed(
                    types, scratch, carried, name, wanted,
                ) {
                    super::application::MemberInstance::Plain(_) => Some(InstanceRead::AsIs),
                    super::application::MemberInstance::Instance { ktype, solution } => {
                        Some(InstanceRead::Solved(StaticInstance::Closed {
                            solution,
                            ktype,
                        }))
                    }
                    super::application::MemberInstance::Undeclared
                    | super::application::MemberInstance::Unfixed { .. }
                    | super::application::MemberInstance::NoInstance(_) => Some(open),
                }
            }
            read => Some(read),
        }
    }

    /// The type the member argument `argument` of the keyworded use at `used` of `shape` is wanted
    /// at, written over names: the one the candidate the instance the reading frame holds runs
    /// there wants it at, or the one every candidate it keeps agrees on; `Some(None)` where those
    /// disagree, which reads a member declared at a type as it is. `None` where the instance does
    /// not answer the selection.
    fn member_wanted<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        used: Site,
        argument: u16,
    ) -> Option<Option<Parametric>>
    where
        'a: 'l,
    {
        let (_, picked) = self.use_read(types, scratch, shape, used).answered?;
        let mut agreed: Option<(Parametric, KType)> = None;
        for each in picked {
            let wanted = *each.selectable.wanted.get(usize::from(argument))?;
            let closed = self.resolved_laying(types, scratch, shape.segment(), wanted);
            let closed = types.concrete(closed)?;
            match agreed {
                Some((_, first)) if first != closed => return Some(None),
                Some(_) => {}
                None => agreed = Some((wanted, closed)),
            }
        }
        Some(agreed.map(|(wanted, _)| wanted))
    }

    /// The point each variable of the group of the candidate the keyworded use at `site` of
    /// `shape` selects solves to, by group index, where the load selected one.
    pub fn selection<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> Option<Points<'l>>
    where
        'a: 'l,
    {
        let read = self.use_read(types, scratch, shape, site);
        self.selection_by(types, scratch, shape, read)
    }

    /// [`selection`](Self::selection) over the instance's answer `read`.
    pub fn selection_by<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        read: UseRead<'a, 'l>,
    ) -> Option<Points<'l>>
    where
        'a: 'l,
    {
        let answered = match read.answered {
            Some((SiteAnswer::Arm(_), [picked])) => picked.points,
            _ => None,
        };
        // A selection the load made at every binding is its record's.
        let selected = answered.or_else(|| read.record(|typed| typed.selections));
        self.resolved_laying(types, scratch, shape.segment(), selected)
    }

    /// The content tree of the module the binder at `slot` of `shape` holds, if the load read it.
    pub fn content(&self, shape: &BodyShape<'_>, slot: Slot) -> Option<Tree<'a>> {
        self.at_slot(shape, slot, |typed| typed.contents)
    }

    /// What each parameter of the call by name whose argument sits at `site` of `shape` is solved
    /// from.
    pub fn named_contributions<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> &'l [StaticType<'l>]
    where
        'a: 'l,
    {
        // The load's record, closed under the frame's binding.
        let named: &'l [StaticType<'l>] =
            (self.at_site(shape, site, |typed| typed.by_name)).unwrap_or(&[]);
        let named = self.resolved_laying(types, scratch, shape.segment(), named);
        carried_at_any(scratch, named)
    }

    /// The slots `shape` binds at a statement that some unit's frontier holds.
    pub fn rooting_seams(&self, shape: &BodyShape<'_>) -> &'a [Slot] {
        self.0
            .folds(shape.segment())
            .map_or(&[], |folds| folds.rooting)
    }

    pub fn statement_seams(&self, shape: &BodyShape<'_>) -> &'a [Slot] {
        self.0
            .folds(shape.segment())
            .map_or(&[], |folds| folds.seams)
    }

    /// The sites of the parts `shape` holds in place, sorted: each one's rank is its held slot.
    pub fn held_sites(&self, shape: &BodyShape<'_>) -> &'a [Site] {
        self.0
            .folds(shape.segment())
            .map_or(&[], |folds| folds.held)
    }

    /// The held slot of the part `shape` holds in place at `site`, where it holds one there.
    pub fn held_index(&self, shape: &BodyShape<'_>, site: Site) -> Option<usize> {
        self.held_sites(shape).binary_search(&site).ok()
    }

    /// The parameters of the callable or block `shape`, or the captures of the quote's code
    /// `shape`, some unit beneath it waits on with its entry as its seam.
    pub fn entry_waits(&self, shape: &BodyShape<'_>) -> &'a [Address] {
        self.0
            .folds(shape.segment())
            .map_or(&[], |folds| folds.waits)
    }

    /// How many of `shape`'s slots keep a digest memo.
    pub fn digested_count(&self, shape: &BodyShape<'_>) -> usize {
        self.0.shapes().digested_count(shape.segment())
    }

    /// Where `slot`'s digest memo sits among `shape`'s, if it keeps one.
    pub fn digested_index(&self, shape: &BodyShape<'_>, slot: Slot) -> Option<usize> {
        self.0.shapes().digested_index(shape.segment(), slot)
    }

    /// Where the captures of the module the `:|` at `site` of `shape` views keep their memos.
    pub fn view_slots(&self, shape: &BodyShape<'_>, site: Site) -> Option<ViewSlots<'a>> {
        self.0.shapes().view_slots(shape.segment(), site)
    }

    /// The digested slots the call keyed by `site` of `shape` passes bare.
    pub fn handoffs(&self, shape: &BodyShape<'_>, site: Site) -> &'a [Handoff] {
        self.0.shapes().handoffs(shape.segment(), site)
    }
}

impl UnitFacts<'_> {
    /// Each fact this record holds that the shape cannot place, by what it is found by.
    pub(super) fn held(&self, each: &mut impl FnMut(Held)) {
        for (segment, typed) in self.typed.iter() {
            typed.held(*segment, each);
        }
        for ((root, name), _) in self.paths.iter() {
            each(Held::Path(*root, *name));
        }
    }
}

impl Typed<'_> {
    /// Each site and through fact of `segment` these tables hold, by what it is found by.
    pub(crate) fn held(&self, segment: Segment, each: &mut impl FnMut(Held)) {
        let sites = (self.expressions.iter().map(|(site, _)| *site))
            .chain(self.parts.iter().map(|(site, _)| *site))
            .chain(self.settled.iter().copied())
            .chain(self.instances.iter().map(|(site, _)| *site))
            .chain(self.uses.iter().map(|(site, _)| *site))
            .chain(self.selections.iter().map(|(site, _)| *site))
            .chain(self.by_name.iter().map(|(site, _)| *site))
            .chain(self.routes.iter().map(|(site, _)| *site))
            .chain(self.arms.iter().map(|(site, _)| *site))
            .chain(self.retypes.iter().map(|(site, _)| *site))
            .chain(self.trees.iter().map(|(site, _)| *site))
            .chain(self.picks.iter().map(|(site, _)| *site))
            .chain(self.minting.iter().copied());
        sites.for_each(|site| each(Held::Site(segment, site)));
    }
}

#[cfg(test)]
thread_local! {
    /// How many rigid facts and points reached the run's door: what the door law holds at nothing.
    /// Tests reset it.
    pub(crate) static RIGID_AT_RUN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    /// How many types a frame's binding closed. Tests reset it.
    pub(crate) static REBOUND: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Count a rigid fact or point the run's door met.
#[cfg(test)]
pub(crate) fn rigid_at_run() {
    RIGID_AT_RUN.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
thread_local! {
    /// How many terms the run probed: each a frame's read found its chain lacking, and each a
    /// walk minted whole before it ran. Tests reset it.
    pub(crate) static TERM_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A closer writing in the closed type `bound` gives a level: a quote's code's captures, as its
/// `EVAL` binds them.
struct Capturing<'q, 'tier, 'pair, 'reg, 'door, 'l> {
    types: &'q Chain<'tier, 'pair, 'reg, 'door>,
    lay: BumpAllocator<'l>,
    bound: &'q dyn Fn(usize) -> Option<KType>,
}

impl Closer for Capturing<'_, '_, '_, '_, '_, '_> {
    fn close(&mut self, ty: DeclaredType<Parametric>) -> Closed {
        let binds = super::derived::LevelsOnly(self.bound);
        super::question::rebound(self.types, self.lay, ty, &binds)
    }

    fn concrete(&self, ty: Parametric) -> Option<KType> {
        self.types.concrete(ty)
    }

    fn rebuilt(
        &mut self,
        scheme: crate::type_lattice::Scheme,
        rebounds: super::Rebounds<'_>,
    ) -> Option<DeclaredType<KType>> {
        let (types, scratch) = (self.types, self.lay);
        super::rebuilt_closed(types, scratch, scheme, rebounds, &mut |ty| self.close(ty))
    }
}

impl<'l> Laying<'l> for Capturing<'_, '_, '_, '_, '_, 'l> {
    type Lay = BumpAllocator<'l>;

    fn lay(&self) -> BumpAllocator<'l> {
        self.lay
    }
}

/// The run's closer over a frame's binding: each type written in by the program's fact domain
/// under what the reading frame's slots bound, read in the body `segment`, laying what it copies
/// in `lay`.
struct Rebinding<'q, 'x, 'tier, 'pair, 'reg, 'door, L> {
    rebinds: &'q dyn super::site::Rebinds,
    binding: Binding<'q>,
    segment: Segment,
    types: &'q Chain<'tier, 'pair, 'reg, 'door>,
    scratch: BumpAllocator<'x>,
    lay: Option<L>,
}

impl<L> Closer for Rebinding<'_, '_, '_, '_, '_, '_, L> {
    /// `ty` with the frame's binding written in. A type the chain does not resolve is left as it
    /// is.
    fn close(&mut self, ty: DeclaredType<Parametric>) -> Closed {
        if !self.types.resolves(ty) {
            return Closed::Same;
        }
        let rebound =
            (self.rebinds).rebound(self.types, self.scratch, self.segment, self.binding, ty);
        #[cfg(test)]
        if let Closed::Type(_) = rebound {
            REBOUND.with(|count| count.set(count.get() + 1));
        }
        rebound
    }

    fn concrete(&self, ty: Parametric) -> Option<KType> {
        self.types.concrete(ty)
    }

    fn rebuilt(
        &mut self,
        scheme: crate::type_lattice::Scheme,
        rebounds: super::Rebounds<'_>,
    ) -> Option<DeclaredType<KType>> {
        let (types, scratch) = (self.types, self.scratch);
        super::rebuilt_closed(types, scratch, scheme, rebounds, &mut |ty| self.close(ty))
    }
}

impl<'l> Laying<'l> for Rebinding<'_, '_, '_, '_, '_, '_, BumpAllocator<'l>> {
    type Lay = BumpAllocator<'l>;

    fn lay(&self) -> BumpAllocator<'l> {
        self.lay.expect("a laying read takes a scratch")
    }
}

/// What a call reads once at its keyworded use ([`FactsOver::use_read`]): the use's facts in the
/// load's record, and what the instance the reading frame holds answers there beside the
/// candidates it runs.
#[derive(Clone, Copy)]
pub struct UseRead<'a, 'l> {
    site: Site,
    /// The facts of the use's body in the record holding the use.
    typed: Option<&'a Typed<'a>>,
    /// `None` where the frame holds no instance owning the use's selection.
    answered: Option<(SiteAnswer, &'l [Picked<'a, 'l>])>,
}

impl<'a> UseRead<'a, '_> {
    /// The entry at the use's site of the record's site-keyed `table`.
    fn record<V: Copy + 'a>(
        &self,
        table: impl FnOnce(&Typed<'a>) -> Keyed<'a, (Site, V)>,
    ) -> Option<V> {
        table(self.typed?).get(self.site).map(|(_, value)| *value)
    }
}

/// One candidate an instance's selection runs: where its asks sit, its verdict, and an *always*
/// one's points, closed.
#[derive(Clone, Copy)]
struct Picked<'a, 'l> {
    selectable: &'a Selectable<'a>,
    verdict: Verdict,
    points: Option<Points<'l>>,
    question: &'a SiteQuestion<'a>,
}

/// The candidates a selection's `answer` runs, by their index among `question`'s, each beside its
/// verdict: the one it selects, *always*, or those it keeps in list order. `None` for an answer
/// that runs none.
fn runs(
    question: &SiteQuestion<'_>,
    answer: SiteAnswer,
) -> Option<smallvec::SmallVec<[(u16, Verdict); 4]>> {
    match answer {
        SiteAnswer::Arm(arm) => Some(smallvec::SmallVec::from_slice(&[(
            arm - 1,
            Verdict::Always,
        )])),
        SiteAnswer::Kept { kept, always, .. } => {
            let count = (question.arms.len() as u16).min(64);
            Some(
                (0..count)
                    .filter(|at| kept & 1 << at != 0)
                    .map(|at| match always & 1 << at != 0 {
                        true => (at, Verdict::Always),
                        false => (at, Verdict::Maybe),
                    })
                    .collect(),
            )
        }
        _ => None,
    }
}

/// The candidate at `at` of a selection `question`.
fn selectable<'a>(question: &SiteQuestion<'a>, at: u16) -> &'a Selectable<'a> {
    match question.arms[1 + usize::from(at)] {
        Payload::Candidate(candidate) => candidate,
        _ => unreachable!("a selection question's arms past its layout are its candidates"),
    }
}

/// The points `candidate` of the selection `question` solves its group to where an instance takes
/// it as admitted, laid in `scratch`, written over names: a point the load read exact is the type
/// itself, closed where it is; one it could not, but whose slot's carried argument points it, is
/// that argument's type; any other the binding makes exact. `None` where its group does not solve.
fn answered_points<'l>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'l>,
    question: &SiteQuestion<'_>,
    candidate: &Selectable<'_>,
) -> Option<Points<'l>> {
    let asks = candidate.points?;
    let points = scratch.fill(asks.len(), |at| {
        let ask = asks[at];
        let exact = question.asks[usize::from(ask.exact)].rigid;
        match (exact, ask.carried) {
            (Some(Answer::Bool(true)), _) | (_, None) => match types.concrete(ask.upper) {
                Some(point) => Point::Closed(point),
                None => Point::Rigid(ask.upper),
            },
            (_, Some(slot)) => Point::Carried(slot),
        }
    });
    Some(Points {
        points,
        variables: &[],
    })
}

/// Whether `contribution` is closed at `Any`: a rigid one a binding closed so, since the load lays
/// an argument whose upper end is `Any` as the carried type.
fn closed_to_any(contribution: &StaticType<'_>) -> bool {
    matches!(contribution, Static::Closed(closed) if *closed == KType::ANY)
}

/// `contributions` with each closed `Any` read as the carried type, as the load lays an argument
/// whose upper end is `Any`: a rigid contribution a binding closes to `Any` leaves its solve to the
/// call.
fn carried_at_any<'l>(
    scratch: BumpAllocator<'l>,
    contributions: &'l [StaticType<'l>],
) -> &'l [StaticType<'l>] {
    if !contributions.iter().any(closed_to_any) {
        return contributions;
    }
    scratch.fill(contributions.len(), |at| match contributions[at] {
        ref contribution if closed_to_any(contribution) => Static::Unknown,
        contribution => contribution,
    })
}

/// `narrowing` with each candidate the load solved left to the call to admit and solve, laid in
/// `scratch`: an *always* one with points is *maybe*, with none, and a selected one the use's
/// one *maybe* candidate. What the load kept, and how it ranks them, stands.
fn solved_at_call<'l>(scratch: BumpAllocator<'l>, narrowing: Narrowing<'l>) -> Narrowing<'l> {
    let kept = match narrowing {
        Narrowing::Full => return Narrowing::Full,
        Narrowing::Selected(coordinate) => Kept {
            entries: scratch.fill(1, |_| (Candidate::One(coordinate), Verdict::Maybe)),
            points: scratch.fill(1, |_| Points::NONE),
            elimination: Elimination::Open,
        },
        Narrowing::Kept(kept) => {
            let solved = |at: usize| !kept.points[at].points.is_empty();
            Kept {
                entries: scratch.fill(kept.entries.len(), |at| match kept.entries[at] {
                    (candidate, Verdict::Always) if solved(at) => (candidate, Verdict::Maybe),
                    entry => entry,
                }),
                points: scratch.fill(kept.points.len(), |at| match solved(at) {
                    true => Points::NONE,
                    false => kept.points[at],
                }),
                elimination: kept.elimination,
            }
        }
    };
    Narrowing::Kept(resident(scratch, kept))
}
