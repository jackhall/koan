//! The **stored site question**: a rule the walk decides at a site, laid whole in the record of
//! the unit that owns the site, so a binding decides it again with no walk
//! ([design/binding-graph.md](../../design/binding-graph.md)).
//!
//! A rule's decision is split four ways: its **asks**, the [`Question`]s it may ask, written over
//! names; a **decision function**, [`decide`], from the asks' answers to its outcome, a
//! [`SiteAnswer`] that holds no type; and one **payload** per outcome, what the outcome writes,
//! laid at the load over names. The walk decides at the load with each ask answered rigidly by its
//! door, and a binding decides with each ask answered under it, by the same function, so the two
//! cannot drift. The decision builds no type and reads no binding: every type an outcome needs is
//! its payload's, written over names and applied with the binding written in where it is read.
//! A refusal is a payload too, a [`ShapeError`] template over names a reader words with the
//! binding written in: [`SiteAnswer::Refused`] names its arm, chosen in the words the walk would
//! refuse in under that binding.
//!
//! A use's selection is also kept as a value, [`Selection`], beside the call's type and its
//! content tree's call: what each candidate does at the call ([`Selection::runs`]) is read from
//! the same decision under every answer an undecided ask may give, so the call's type and the
//! candidate a binding unfolds follow the walk's own selection.
//!
//! A rule the walk does not yet lay decides nothing ([`SiteAnswer::Undecided`]), and its readers
//! keep the walk's own record.

use crate::memory::{BumpAllocator, Lays, collect};
use crate::type_lattice::{DeclaredType, Interval, KKind, KType, Parametric, Verdict};
use crate::values::Chain;
use crate::values::digest::DigestHasher;

use super::keyed::KeyedEntry;
use super::question::{Answer, Posed, Question, Substitute, answer_fed};
use super::records::{Fixed, Restamp};
use super::rigid::rigid_answer;
use super::runs::{GuardKind, Run};
use super::shape::{Segment, ShapeError, Site, Slot};
use super::typed::{CandidateRoute, StaticInstance, StaticType};

/// A rule decided at `site` of `segment`'s body, beside its asks and the payload of each outcome.
#[derive(Clone, Copy, Debug)]
pub struct SiteQuestion<'a> {
    pub segment: Segment,
    pub site: Site,
    pub rule: SiteRule,
    /// What the rule may ask, by index, each beside its door's rigid answer at the load:
    /// [`decide`] asks an index only where the answers before it leave it needed.
    pub asks: &'a [Posed<'a>],
    /// Each outcome's payload, by the index its [`SiteAnswer`] names.
    pub arms: &'a [Payload<'a>],
    /// The outcome the walk took at the load: [`decide`] under each ask's rigid answer.
    pub taken: SiteAnswer,
}

impl SiteQuestion<'_> {
    /// Hand `each` every type the question names: its asks', and its payloads', over names.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        for posed in self.asks {
            posed.question.each_type(each);
        }
        for arm in self.arms {
            arm.each_type(each);
        }
    }
}

impl Payload<'_> {
    /// Hand `each` every type the payload names, over names.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        use super::record_types::Named;
        match self {
            Payload::Candidate(candidate) => {
                for point in candidate.points.into_iter().flatten() {
                    each(point.upper.into());
                }
                let passed = (candidate.passes.iter()).flat_map(|passes| passes.slots.iter());
                for (_, passing) in passed {
                    match passing {
                        Passing::Fixed(fixed) => fixed.each_type(&mut |kt: KType| each(kt.into())),
                        Passing::Type(ty) => each((*ty).into()),
                        Passing::Carried(typed) => {
                            each(typed.lower.into());
                            each(typed.upper.into());
                        }
                        Passing::Point(_) => {}
                    }
                }
            }
            Payload::Restamp(restamp) => restamp.each_named(each),
            Payload::Type(ty) => each(*ty),
            Payload::Contributions(contributions) => contributions.each_named(each),
            Payload::Instance(instance) => instance.each_named(each),
            Payload::Refusal(refusal) => refusal.each_named(each),
            Payload::Selection(_)
            | Payload::Routing(_)
            | Payload::Checks(_)
            | Payload::Arms(_)
            | Payload::Guard(_) => {}
        }
    }
}

impl KeyedEntry for SiteQuestion<'_> {
    type Key = (Segment, Site, SiteRule);

    fn key(&self) -> Self::Key {
        (self.segment, self.site, self.rule)
    }
}

/// What a frame's instances answer a stored site question: the answer the instance a frame holds
/// gives the question of `rule` at `site` of `segment`, read through `handles`, the frame's handle
/// words. `None` where no instance the frame holds owns the question, which leaves the load's
/// record to the reader. A binding graph's instances implement it; `scope` names no graph.
pub trait SiteAnswers {
    fn answered(
        &self,
        segment: Segment,
        site: Site,
        rule: SiteRule,
        handles: Handles<'_>,
    ) -> Option<SiteAnswer>;
}

/// The handle words a frame answers a site question through: its own slab, at the offsets the
/// graph's layout fixes for the body owning its units, and the words each frame around the
/// callable it runs held when that callable was born, innermost first. A question an encloser's
/// node owns is answered through the encloser's words.
#[derive(Clone, Copy, Default)]
pub struct Handles<'a> {
    pub slab: &'a [std::cell::Cell<u64>],
    pub enclosers: &'a [EncloserWords<'a>],
}

/// The handle slab a frame of the owning body `body` held when a callable was born in it, as it
/// stood then.
#[derive(Clone, Copy, Debug)]
pub struct EncloserWords<'a> {
    pub body: Segment,
    pub words: &'a [u64],
}

/// What a frame's binding writes into a type a base record names: `ty`, read in the body
/// `segment`, with `binding` written in by the program's fact domain over the load's derived
/// names. Module substitution included: a path class's variable is read through the class recipes
/// the fold laid and the modules the binding binds. A binding graph's instances implement it;
/// `scope` names no graph.
pub trait Rebinds {
    fn rebound(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        segment: Segment,
        binding: super::facts::Binding<'_>,
        ty: DeclaredType<Parametric>,
    ) -> super::Closed;

    /// Whether `ty` names a derived name the load did not mint: a layer's, which only a layer's
    /// table closes.
    #[cfg(test)]
    fn foreign(&self, _types: &Chain<'_, '_, '_, '_>, _ty: DeclaredType<Parametric>) -> bool {
        false
    }
}

/// The rule a site question decides: one per discrete door a binding can decide otherwise than
/// the load. A body's class is no site question: the load's conservative class holds at every
/// binding, since only the base's folds decide caching.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum SiteRule {
    /// A keyworded use: each candidate's verdict, a builtin's needs and its instance slots, the
    /// ranking, and the points of the one selected; so what the call keeps, and at a tail call how
    /// each kept candidate routes its frame.
    Selection,
    /// An argument's contribution to a solve, exact or left to the call.
    Contribution,
    /// An instance wanted at a function type: whether one lies under it, and the instance.
    InstanceAtFunction,
    /// A member of a module the load cannot see read at an instance site: whether it has an
    /// instance there.
    MemberInstance,
    /// `ATTR`'s member retype.
    MemberRetype,
    /// A call by name's route from the tail of its frame's body; a keyworded use's routes with
    /// its selection.
    TailRoute,
    /// A call by name: whether its argument arrives, its layout and its verdict, and what each
    /// argument contributes to the callee's solve.
    ByName,
    /// An application over names.
    Application,
    /// A registration's overlap with a builtin.
    Overlap,
    /// A `:|` view's checks.
    View,
    /// A `:|` over a closed content tree.
    ViewTree,
    /// A `:!` hold's or an annotated `LET`'s refusal and settlement.
    Hold,
    /// What a `:!` hold or an annotated `LET` re-stamps at.
    HoldRetype,
    /// A part's type through its content tree.
    PartTree,
    /// A `USING … SCOPE` header's picks.
    Picks,
    /// `EVAL code -> T`.
    Eval,
    /// A splice or a spread.
    Splice,
    /// Which arms of a `MATCH` or `TRY` run.
    ArmsRun,
    /// What an arm's `it` is re-stamped at.
    ArmRestamp,
    /// Whether `(head arg)` is a read, and of which container.
    IsRead,
    /// A read's refusal.
    ReadRefusal,
    /// Whether a path's root is exact.
    RootExact,
    /// Whether a binder is a module binder.
    ModuleBinder,
    /// A deferred callable body that can never be code.
    NeverCode,
    /// A value member read through a module the run builds: a carried type, a view or a path.
    SignatureValue,
    /// A signature meet's member, over the paths a module parameter's pins read.
    SignatureMeet,
    /// A repeated `MATCH … WITH` guard.
    RepeatedGuard,
    /// The return check.
    ReturnCheck,
}

impl SiteRule {
    /// Every rule, in declaration order.
    pub const ALL: [SiteRule; 28] = [
        SiteRule::Selection,
        SiteRule::Contribution,
        SiteRule::InstanceAtFunction,
        SiteRule::MemberInstance,
        SiteRule::MemberRetype,
        SiteRule::TailRoute,
        SiteRule::ByName,
        SiteRule::Application,
        SiteRule::Overlap,
        SiteRule::View,
        SiteRule::ViewTree,
        SiteRule::Hold,
        SiteRule::HoldRetype,
        SiteRule::PartTree,
        SiteRule::Picks,
        SiteRule::Eval,
        SiteRule::Splice,
        SiteRule::ArmsRun,
        SiteRule::ArmRestamp,
        SiteRule::IsRead,
        SiteRule::ReadRefusal,
        SiteRule::RootExact,
        SiteRule::ModuleBinder,
        SiteRule::NeverCode,
        SiteRule::SignatureValue,
        SiteRule::SignatureMeet,
        SiteRule::RepeatedGuard,
        SiteRule::ReturnCheck,
    ];
}

/// A site question's outcome, holding no type: which payload it takes, or the discrete answer a
/// reader acts on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SiteAnswer {
    /// The payload at this index: a candidate, a `MATCH` arm, a member kind, a restamp's how.
    Arm(u16),
    /// The candidates a call still selects among, a bit each by their arm's order, beside those
    /// that always admit and whether a table ranks them.
    Kept {
        kept: u64,
        always: u64,
        ranked: bool,
    },
    Verdict(Verdict),
    Bool(bool),
    /// The refusal template at this index among the site's payloads.
    Refused(u16),
    /// No answer here: the reader leaves the outcome to the value at run.
    Undecided,
}

impl SiteAnswer {
    /// The arm the answer follows: an [`Arm`](SiteAnswer::Arm)'s, or a [`Bool`](SiteAnswer::Bool)'s
    /// as two arms, no first. `None` otherwise, so an undecided answer follows no arm.
    pub fn arm(self) -> Option<u16> {
        match self {
            SiteAnswer::Arm(arm) => Some(arm),
            SiteAnswer::Bool(holds) => Some(u16::from(holds)),
            SiteAnswer::Kept { .. }
            | SiteAnswer::Verdict(_)
            | SiteAnswer::Refused(_)
            | SiteAnswer::Undecided => None,
        }
    }
}

/// What one outcome of a site question writes, laid at the load over names: a reader applies it
/// with its binding written in.
#[derive(Clone, Copy, Debug)]
pub enum Payload<'a> {
    /// A use's selection question's layout: its first arm.
    Selection(Selecting),
    /// A candidate a use may select, by where its asks sit.
    Candidate(&'a Selectable<'a>),
    /// A `MATCH` arm's or a retype's restamp.
    Restamp(Restamp),
    /// A type the outcome hands back.
    Type(DeclaredType<Parametric>),
    /// What each argument of a use contributes to its solves, by position, as the walk records it.
    Contributions(&'a [StaticType<'a>]),
    /// A call by name's route from the tail of its frame's body, by where its asks sit
    /// ([`SiteRule::TailRoute`]): its one arm.
    Routing(Routing),
    /// An instance the load makes, at its solution over names.
    Instance(StaticInstance<'a>),
    /// A call by name's checks of its instance slots ([`SiteRule::ByName`]): each ask, the
    /// answer that refuses the call, and the arm of the refusal it is refused in.
    Checks(&'a [InstanceCheck]),
    /// A refusal template: its types over names, worded with the binding written in.
    Refusal(&'a ShapeError<'a>),
    /// A `MATCH`'s or `TRY`'s arms question's layout ([`SiteRule::ArmsRun`]): its first arm.
    Arms(ArmsLayout),
    /// An arm of a `MATCH` or `TRY`, by where its asks sit.
    Guard(GuardAsks),
}

/// The layout of a `MATCH`'s or `TRY`'s arms question ([`SiteRule::ArmsRun`]) over `count` arms.
/// Its asks run, in order: whether the scrutinee's upper end is `Never`, `arriving` of them, from
/// 0; each arm's, as its [`GuardAsks`] places them; and from `ranks`, whether the type of each of
/// the `ranked` arms that rank by their type lies under each other's ([`ArmsLayout::under`]).
///
/// Its arms are the layout, each arm's [`GuardAsks`] by its order from 1, then the refusals of a
/// binding that refuses the `MATCH`: `NoAdmittingArm`, then `AmbiguousArms` at each count of arms
/// ranked alike ([`ArmsLayout::ambiguous`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ArmsLayout {
    pub count: u16,
    pub arriving: u16,
    pub ranks: u16,
    pub ranked: u16,
}

impl ArmsLayout {
    /// The arm of the refusal of a `MATCH` no arm is left to.
    pub fn no_admitting(&self) -> u16 {
        self.count + 1
    }

    /// The arm of the refusal of a `MATCH` whose `count` arms always admit, ranked alike.
    pub fn ambiguous(&self, count: u32) -> u16 {
        self.count + 2 + count as u16
    }

    /// The ask whether the type of the ranked arm `a` lies under that of `b`, `a` and `b` apart.
    pub fn under(&self, a: u16, b: u16) -> u16 {
        let b = b - u16::from(b > a);
        self.ranks + a * (self.ranked - 1) + b
    }
}

/// One arm of a `MATCH` or `TRY` as its arms question reads it: its guard's kind and where its
/// asks sit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GuardAsks {
    pub guard: GuardKind,
    /// The ask whether the scrutinee can never satisfy what the guard admits by: `None` for `_`
    /// under `WITH`, which always admits, and for a guard the load cannot read, which may.
    pub never: Option<u16>,
    /// The ask whether the scrutinee's upper end fits what the guard admits by: `None` for a value
    /// guard, which never always admits.
    pub always: Option<u16>,
    /// Its index among the arms that rank by their type: a type guard the load reads.
    pub rank: Option<u16>,
}

/// The layout of a keyworded use's selection question ([`SiteRule::Selection`]). Its asks run, in
/// order: whether each argument the load cannot read closed is `Never`, from 0, `arriving` of them;
/// whether each argument a solve reads is `Any`, `open` of them; each candidate's, as its
/// [`Selectable`] places them; from `agrees`, whether each member argument is declared at a type
/// and whether each pair of candidates wants it at one type ([`Selecting::plain`],
/// [`Selecting::agree`]); and from `ranks`, whether each ranked candidate strictly outranks each
/// other at each of `classes` classes, by [`Selecting::outranks`].
///
/// Its arms past its candidates' are the refusals of a binding that refuses the use, from
/// `refusals`: `NoAdmittingCandidate`, `NoInstanceAtCandidates`, then `Ambiguous` at each count of
/// candidates ranked alike from two ([`Selecting::ambiguous`]); the load's own and `split`'s
/// follow, then each candidate's, where its [`Selectable`] places them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selecting {
    /// How many candidates the use's list holds, those the load dropped among them.
    pub listed: u16,
    /// Whether the list is one function, which the text selects where the use never runs.
    pub single: bool,
    pub arriving: u16,
    pub open: u16,
    /// How many candidates have a shape a table ranks by.
    pub ranked: u16,
    pub classes: u16,
    pub ranks: u16,
    /// The arm of the first refusal, past the candidates'.
    pub refusals: u16,
    /// Where the load dropped a candidate over an instance argument's refusal, the arm of the
    /// refusal a use left with none words: the lone candidate's own, else
    /// `NoInstanceAtCandidates`.
    pub refused: Option<u16>,
    /// The first candidate the load dropped over a builtin's need: its position on the list,
    /// beside the arm of what its argument lacks, where the need was narrowed by a name.
    pub dropped: Option<(u16, Option<u16>)>,
    /// How many instance arguments are members of a module the load cannot see, each wanted at
    /// its candidate's [`Selectable::wanted`]; beside where their asks start, by
    /// [`Selecting::plain`] and [`Selecting::agree`], and the arm of the refusal of a use whose
    /// candidates left want a quantified one at different types (`AmbiguousInstance`).
    pub members: u16,
    pub agrees: u16,
    pub split: Option<u16>,
}

impl Selecting {
    /// The arm of the refusal of a use no candidate admits where none was dropped over an
    /// instance argument or a need narrowed by a name.
    pub fn no_admitting(&self) -> u16 {
        self.refusals
    }

    /// The arm of the refusal of a use of several candidates, each dropped where none takes a
    /// quantified function argument.
    pub fn no_instance_at(&self) -> u16 {
        self.refusals + 1
    }

    /// The arm of the refusal of a use whose `count` candidates rank alike, from two.
    pub fn ambiguous(&self, count: u32) -> u16 {
        self.refusals + count as u16
    }

    /// The ask whether the member argument `member` is declared at a type, not a scheme, among
    /// a selection of `count` candidates.
    pub fn plain(&self, member: u16, count: u16) -> u16 {
        self.agrees + member * (1 + count * count.saturating_sub(1) / 2)
    }

    /// The ask whether the candidates `a` and `b`, `a` before `b` among `count`, want the member
    /// argument `member` at one type.
    pub fn agree(&self, member: u16, count: u16, a: u16, b: u16) -> u16 {
        let before = a * (2 * count - a - 1) / 2;
        self.plain(member, count) + 1 + before + (b - a - 1)
    }

    /// The ask whether the ranked candidate `a` strictly outranks `b` at `class`, `a` and `b`
    /// apart.
    pub fn outranks(&self, class: u16, a: u16, b: u16) -> u16 {
        let len = self.ranked;
        let b = b - u16::from(b > a);
        self.ranks + (class * len + a) * (len - 1) + b
    }
}

/// One candidate a use's selection may keep, as the load kept it: where its asks sit and what the
/// text fixes of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selectable<'a> {
    /// Its position on the use's list.
    pub position: u16,
    /// Whether it is one function; a spread is not.
    pub one: bool,
    pub builtin: bool,
    /// Its index among the ranked candidates, where it has a shape.
    pub rank: Option<u16>,
    /// The ask its verdict is; `None` for a candidate whose shape the load cannot read, which is
    /// *maybe*.
    pub verdict: Option<u16>,
    /// Where the use has a member argument ([`Selecting::split`]) and an argument reads a name
    /// declared `Any`, the ask of its verdict with each such name read at its ends, as the text
    /// declares it: the candidates a binding leaves a member argument's instance to.
    pub ended: Option<u16>,
    /// What each instance argument's slot asks of it, in the order the walk asks: no drops it.
    pub checks: &'a [InstanceCheck],
    /// Its builtin's needs, each slot's: one an argument's lower end lies outside drops it.
    pub needs: &'a [Need],
    /// What each need's argument may lack, where its [`Need`] places them.
    pub lacks: &'a [Lack],
    /// Where a surfaced key holds it, whether the key is exactly its head: a surfaced candidate is
    /// at most *maybe*, and *never* only where it is exact.
    pub surfaced: Option<bool>,
    /// Each variable of its group's point: `None` where some variable has none at every binding.
    pub points: Option<&'a [PointAsk]>,
    /// How it routes its frame, where the use is a callable body's tail call.
    pub route: Option<Routing>,
    /// Where it is a surfaced signature head in filled code, the ask whether the head is closed:
    /// a frame retypes its value to a closed head's return, so run alone, its return is exactly
    /// the head's.
    pub retyped: Option<u16>,
    /// Where it calls a callable body of a closed registration, what its call passes there.
    pub passes: Option<Passes<'a>>,
    /// The type it wants each member argument at ([`Selecting::members`]), written over names.
    pub wanted: &'a [Parametric],
}

/// What a candidate's call passes its callee, written over names: the mapping a call gives the
/// names at its callee's entry, laid for every candidate a binding may select.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Passes<'a> {
    /// The callee's body.
    pub callee: Segment,
    /// What it passes at each callee slot it passes anything at, sorted by slot.
    pub slots: &'a [(Slot, Passing)],
}

/// What a call passes at one of its callee's slots, written over names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Passing {
    /// Load content, or the caller's own slot passed on bare.
    Fixed(Fixed),
    /// The candidate's point at its group's variable, by index: its upper end where its exact
    /// ask is yes, else nothing.
    Point(u16),
    /// A type value written over names.
    Type(Parametric),
    /// The static type of an argument at a parameter declared `Any`, written over names: the type
    /// its value carries under a binding that makes it exact.
    Carried(Interval),
}

impl Selectable<'_> {
    /// This candidate with its slices laid again through `lay`.
    pub fn laid<'to>(&self, lay: impl Lays<'to>) -> Selectable<'to> {
        Selectable {
            checks: collect(lay, self.checks.iter().copied()),
            wanted: collect(lay, self.wanted.iter().copied()),
            needs: collect(lay, self.needs.iter().copied()),
            lacks: collect(lay, self.lacks.iter().copied()),
            points: (self.points).map(|points| collect(lay, points.iter().copied())),
            passes: (self.passes).map(|passes| Passes {
                slots: collect(lay, passes.slots.iter().copied()),
                ..passes
            }),
            ..*self
        }
    }

    /// The route the candidate takes where the decision keeps it, `solved` where it selects it or
    /// keeps it *always*: a scheme's return is read at its group's points, so it keeps the frame's
    /// contract only where the call is solved and every point is exact. `None` where the use is
    /// no tail call.
    pub fn routed(&self, solved: bool, asks: &mut dyn Asks) -> Option<CandidateRoute> {
        let routing = self.route?;
        let pointed = |asks: &mut dyn Asks| {
            let points = self.points.filter(|_| solved)?;
            for point in points {
                if asks.ask(point.exact) != Some(Answer::Bool(true)) {
                    return None;
                }
            }
            Some(())
        };
        let kept = match routing.keeps {
            Some((_, true)) if pointed(asks).is_none() => None,
            _ => routing.kept(asks),
        };
        Some(CandidateRoute {
            kept,
            hop: routing.hop,
        })
    }
}

/// How a candidate routes its frame from the use in the tail of the frame's callable body: the
/// ask whether its return keeps the frame's contract, and whether the call hops into the frame's
/// own body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Routing {
    /// The ask whether the callee's return, a scheme's at its group's points, fits the contract —
    /// `None` where the callee is no function, which keeps nothing — beside whether the callee is
    /// a scheme. `None` where the call decides: a builtin's, a surfaced key's, a spread's, one
    /// whose shape the load cannot read, or one into a frame whose contract is open.
    pub keeps: Option<(Option<u16>, bool)>,
    /// Where a call by name's callee is read off its head, the ask whether the head is exactly
    /// its type: the return is read only where it is, else the call decides.
    pub exact: Option<u16>,
    pub hop: bool,
}

impl Routing {
    /// Whether the callee's return keeps the frame's contract, a scheme's points already read
    /// exact: `None` where the call decides.
    pub fn kept(&self, asks: &mut dyn Asks) -> Option<bool> {
        let (fits, _) = self.keeps?;
        if let Some(exact) = self.exact
            && asks.ask(exact) != Some(Answer::Bool(true))
        {
            return None;
        }
        match fits.map(|fits| asks.ask(fits)) {
            None => Some(false),
            Some(Some(Answer::Bool(kept))) => Some(kept),
            Some(_) => None,
        }
    }
}

/// One slot's need of a builtin candidate: the ask whether its argument's lower end lies outside
/// it, or outside a narrower need where the gate's ask gives its answer; beside where its
/// candidate's [`Lack`]s for the slot sit, `[from, to)`, which a use the need leaves with no
/// candidate is refused by.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Need {
    pub outside: u16,
    pub narrowed: Option<(u16, Answer, u16)>,
    pub lacks: (u16, u16),
}

/// One name a need narrowed by the names a field read reads may find its argument lacking, in
/// the order written: the ask whether the argument names no field by it — `None` where it lacks
/// it at every binding — beside the arm of the refusal naming it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Lack {
    pub ask: Option<u16>,
    pub refusal: u16,
}

/// One check an instance argument's slot makes of a candidate: the ask, whether an argument the
/// slot is solved from is `Any`, whether its interval is one type (`exact`; undecided, the
/// candidate is at most *maybe*) or whether an instance lies under the slot; beside the answer
/// that drops it, `when`, and the arm of the refusal a lone candidate so dropped is refused in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InstanceCheck {
    pub ask: u16,
    pub when: bool,
    pub exact: bool,
    pub refusal: u16,
}

/// One variable of a candidate's group: the ask whether its interval is one type, which is then its
/// point, `upper` written over names; else the slot whose carried argument points it, where one
/// does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PointAsk {
    pub exact: u16,
    pub upper: Parametric,
    pub carried: Option<u16>,
}

/// How a decision function has its asks answered: at the load, each ask's rigid answer by its
/// door; under a binding, each ask's answer with the binding written in.
pub trait Asks {
    /// The answer to the ask at `index` of the site question's: `None` where it is undecided, as
    /// a door at the load leaves a question no rigid answer holds.
    fn ask(&mut self, index: u16) -> Option<Answer>;
}

/// `question`'s outcome from the answers `asks` gives, each ask made only where the answers before
/// it leave it needed: the same answers in the same order give the same asks. A rule the walk does
/// not yet lay is undecided.
pub fn decide(question: &SiteQuestion<'_>, asks: &mut dyn Asks) -> SiteAnswer {
    match question.rule {
        SiteRule::Contribution | SiteRule::ByName => contributed(question, asks),
        SiteRule::Selection => selection(question, asks),
        SiteRule::ArmsRun => arms_chosen(question, asks),
        // Its one ask is whether an instance lies under the wanted type: no refuses
        // (`NoInstance`, the second arm), else the instance as laid.
        SiteRule::InstanceAtFunction => match asks.ask(0) {
            Some(Answer::Bool(false)) => SiteAnswer::Refused(1),
            _ => SiteAnswer::Arm(0),
        },
        // Whether the callee's return keeps the frame's contract; undecided, the call decides.
        SiteRule::TailRoute => {
            let [Payload::Routing(routing)] = question.arms else {
                panic!("a tail route's one arm is its routing");
            };
            match routing.kept(asks) {
                Some(kept) => SiteAnswer::Bool(kept),
                None => SiteAnswer::Undecided,
            }
        }
        // A member read whose candidate may run, over a module read exactly, re-stamps its member
        // as its restamp ask answers: the arm by the answer's index. Else the member is read at
        // its value. An arm's `it` re-stamps as its one ask answers.
        SiteRule::MemberRetype | SiteRule::ArmRestamp => {
            for (index, posed) in (0..).zip(question.asks) {
                match (posed.question, asks.ask(index)) {
                    (Question::Verdict { .. }, Some(Answer::Verdict(Verdict::Never))) => break,
                    (Question::Verdict { .. }, _)
                    | (Question::Same { .. }, Some(Answer::Bool(true))) => {}
                    (Question::Restamp { .. }, Some(how)) => {
                        return SiteAnswer::Arm(how.index_in(&posed.question) as u16);
                    }
                    _ => break,
                }
            }
            SiteAnswer::Undecided
        }
        // An application over names, or a member's instance, whose rule a binding leaves
        // undefined, a splice or spread whose operand it puts outside a list, an `EVAL` whose
        // operand or result it puts outside what each must satisfy, a deferred body it puts outside
        // all code, a read whose argument it puts outside what reads its container, a body whose
        // value it puts outside its declared return: refused at the first such ask, in its words.
        // An operand that never arrives runs nothing, so refuses nothing after it.
        SiteRule::Application
        | SiteRule::MemberInstance
        | SiteRule::Splice
        | SiteRule::Eval
        | SiteRule::NeverCode
        | SiteRule::ReadRefusal
        | SiteRule::ReturnCheck => first_refusal(question, asks),
        SiteRule::View => viewed(question, asks),
        // A registration a binding makes overlap a builtin overload is refused in its words.
        SiteRule::Overlap => match asks.ask(0) {
            Some(Answer::Overlaps(Some(at))) => SiteAnswer::Refused(at),
            _ => SiteAnswer::Undecided,
        },
        SiteRule::Hold | SiteRule::HoldRetype => held(question, asks),
        // Two guards of one arm set a binding makes one type refuse the later, the first such
        // pair in the order a fresh load reads them.
        SiteRule::RepeatedGuard => (0..question.asks.len() as u16)
            .find(|at| asks.ask(*at) == Some(Answer::Bool(true)))
            .map_or(SiteAnswer::Undecided, SiteAnswer::Refused),
        SiteRule::ViewTree
        | SiteRule::PartTree
        | SiteRule::Picks
        | SiteRule::IsRead
        | SiteRule::RootExact
        | SiteRule::ModuleBinder
        | SiteRule::SignatureValue
        | SiteRule::SignatureMeet => SiteAnswer::Undecided,
    }
}

/// [`SiteRule::Contribution`]'s and [`SiteRule::ByName`]'s outcome, the asks read in the order the
/// walk reads them: an argument's upper end that is `Never` never runs the call, which contributes
/// nothing (the second arm); a layout that can never name a callee's parameters, or a verdict
/// *never*, refuses it (the third, `CallNeverSatisfied`); a call by name's instance slot a binding
/// leaves unfixed, or with no instance under it, refuses it in that slot's words (each
/// [`InstanceCheck`] of its fourth arm); an upper end a solve reads that is `Any` leaves the
/// selection or the solve to the call, where nothing refuses it; else the contributions as laid.
fn contributed(question: &SiteQuestion<'_>, asks: &mut dyn Asks) -> SiteAnswer {
    let never = DeclaredType::Type(Parametric::from(KType::NEVER));
    let checks = match question.arms.get(3) {
        Some(Payload::Checks(checks)) => *checks,
        _ => &[],
    };
    let mut open = false;
    for (index, posed) in (0..).zip(question.asks) {
        if let Some(check) = checks.iter().find(|check| check.ask == index) {
            if asks.ask(index) == Some(Answer::Bool(check.when)) {
                return SiteAnswer::Refused(check.refusal);
            }
            continue;
        }
        match (posed.question, asks.ask(index)) {
            (Question::Same { b, .. }, Some(Answer::Bool(true))) if b == never => {
                return SiteAnswer::Arm(1);
            }
            (Question::Same { .. }, Some(Answer::Bool(true))) => open = true,
            (Question::Defined { .. }, Some(Answer::Bool(false)))
            | (Question::Verdict { .. }, Some(Answer::Verdict(Verdict::Never))) => {
                return SiteAnswer::Refused(2);
            }
            _ => {}
        }
    }
    match open {
        true => SiteAnswer::Undecided,
        false => SiteAnswer::Arm(0),
    }
}

/// The outcome of a rule whose asks each refuse it or not, in the order the walk asks them: the
/// first that refuses decides, the refusal its arm by the refusing asks before it. An operand
/// that is `Never` stops the rule, which then refuses nothing. Else the rule's type is its decided
/// names', no payload's.
fn first_refusal(question: &SiteQuestion<'_>, asks: &mut dyn Asks) -> SiteAnswer {
    let never = DeclaredType::Type(Parametric::from(KType::NEVER));
    let mut refusals = 0;
    for (index, posed) in (0..).zip(question.asks) {
        let refuses = match (posed.question, asks.ask(index)) {
            (Question::Same { b, .. }, answer) if b == never => {
                if answer == Some(Answer::Bool(true)) {
                    return SiteAnswer::Undecided;
                }
                continue;
            }
            (Question::Defined { .. }, answer) => answer == Some(Answer::Bool(false)),
            (Question::LowerEndOutside { .. }, answer) => answer == Some(Answer::Bool(true)),
            (Question::NeverSatisfies { .. }, answer) => answer == Some(Answer::Never),
            (question, _) => unreachable!("{question:?} is no refusing ask"),
        };
        if refuses {
            return SiteAnswer::Refused(refusals);
        }
        refusals += 1;
    }
    SiteAnswer::Undecided
}

/// [`SiteRule::Hold`]'s and [`SiteRule::HoldRetype`]'s outcome, the asks read in the order the
/// walk asks them: a value that never arrives settles and re-stamps nothing; one that can never
/// satisfy the type refuses (the hold's one arm) and re-stamps nothing; an exact module is held at
/// what the run retypes it to, which no ask decides yet. Else the hold settles where its value's
/// upper end fits the type, and re-stamps as its restamp ask answers: the arm by its index.
fn held(question: &SiteQuestion<'_>, asks: &mut dyn Asks) -> SiteAnswer {
    let never = DeclaredType::Type(Parametric::from(KType::NEVER));
    let retype = question.rule == SiteRule::HoldRetype;
    let mut module = false;
    for (index, posed) in (0..).zip(question.asks) {
        match (posed.question, asks.ask(index)) {
            (Question::Same { b, .. }, answer) if b == never => {
                if answer == Some(Answer::Bool(true)) {
                    return match retype {
                        true => SiteAnswer::Undecided,
                        false => SiteAnswer::Bool(false),
                    };
                }
            }
            (Question::NeverSatisfies { .. }, Some(Answer::Never)) => {
                return match retype {
                    true => SiteAnswer::Undecided,
                    false => SiteAnswer::Refused(0),
                };
            }
            (Question::Kind { .. }, answer) => {
                module = answer == Some(Answer::Kind(KKind::Signature));
            }
            (Question::Same { .. }, Some(Answer::Bool(true))) if module => {
                return SiteAnswer::Undecided;
            }
            (Question::Fits { .. }, answer) => {
                return SiteAnswer::Bool(answer == Some(Answer::Bool(true)));
            }
            (Question::Restamp { .. }, Some(how)) => {
                return SiteAnswer::Arm(how.index_in(&posed.question) as u16);
            }
            _ => {}
        }
    }
    SiteAnswer::Undecided
}

/// [`SiteRule::View`]'s outcome, the asks read in the order the walk asks them: an operand that
/// never arrives settles nothing; one that can never be a module refuses (the first arm), and so
/// does a type that is no signature (the second); where the operand's or the type's kind is no
/// signature, nothing settles; an exact operand that never fits the type refuses (the third). Else
/// it settles where it fits.
fn viewed(question: &SiteQuestion<'_>, asks: &mut dyn Asks) -> SiteAnswer {
    let never = DeclaredType::Type(Parametric::from(KType::NEVER));
    let mut fits = None;
    for (index, posed) in (0..).zip(question.asks) {
        match (posed.question, asks.ask(index)) {
            (Question::Same { b, .. }, Some(Answer::Bool(true))) if b == never => {
                return SiteAnswer::Bool(false);
            }
            (Question::NeverSatisfies { .. }, Some(Answer::Never)) => {
                return SiteAnswer::Refused(0);
            }
            (Question::Module { .. }, Some(Answer::Bool(false))) => return SiteAnswer::Refused(1),
            (Question::Kind { .. }, Some(Answer::Kind(kind))) if kind != KKind::Signature => {
                return SiteAnswer::Bool(false);
            }
            (Question::Fits { .. }, answer) => fits = answer,
            (Question::Same { .. }, Some(Answer::Bool(true)))
                if fits == Some(Answer::Bool(false)) =>
            {
                return SiteAnswer::Refused(2);
            }
            _ => {}
        }
    }
    SiteAnswer::Bool(fits == Some(Answer::Bool(true)))
}

/// [`SiteRule::Selection`]'s outcome: [`chosen`], with the one chosen selected only where its
/// points are exact, and an *always* one kept without exact points at most *maybe*.
fn selection(question: &SiteQuestion<'_>, asks: &mut dyn Asks) -> SiteAnswer {
    let [Payload::Selection(layout), arms @ ..] = question.arms else {
        unreachable!("a selection question's first arm is its layout");
    };
    let candidate = |at: u32| match arms[at as usize] {
        Payload::Candidate(candidate) => candidate,
        _ => unreachable!("a selection question's arms past its layout are its candidates"),
    };
    let count = u32::from(layout.refusals) - 1;
    let (kept, mut always, ranked, chosen) = match chosen(layout, count, candidate, asks) {
        Chosen::Outcome(outcome) => return outcome,
        Chosen::Kept {
            kept,
            always,
            ranked,
            chosen,
        } => (kept, always, ranked, chosen),
    };
    let pointed = |asks: &mut dyn Asks, at: u32| {
        (candidate(at).points).is_some_and(|points| {
            points.iter().all(|point| {
                point.carried.is_some() || asks.ask(point.exact) == Some(Answer::Bool(true))
            })
        })
    };
    if let Some(chosen) = chosen.filter(|chosen| pointed(asks, *chosen)) {
        return SiteAnswer::Arm(chosen as u16 + 1);
    }
    for at in 0..count {
        if always & 1 << at != 0 && !pointed(asks, at) {
            always &= !(1 << at);
        }
    }
    SiteAnswer::Kept {
        kept,
        always: always & kept,
        ranked,
    }
}

/// [`SiteRule::ArmsRun`]'s outcome: [`armed`], its chosen arm by its order past the layout.
fn arms_chosen(question: &SiteQuestion<'_>, asks: &mut dyn Asks) -> SiteAnswer {
    let [Payload::Arms(layout), guards @ ..] = question.arms else {
        unreachable!("an arms question's first arm is its layout");
    };
    let guard = |at: u16| match guards[usize::from(at)] {
        Payload::Guard(guard) => guard,
        _ => unreachable!("an arms question's arms past its layout are its arms'"),
    };
    match armed(layout, guard, asks) {
        Chosen::Outcome(outcome) => outcome,
        Chosen::Kept {
            chosen: Some(chosen),
            ..
        } => SiteAnswer::Arm(chosen as u16 + 1),
        Chosen::Kept {
            kept,
            always,
            ranked,
            chosen: None,
        } => SiteAnswer::Kept {
            kept,
            always,
            ranked,
        },
    }
}

/// The walk's own choice of which arms of a `MATCH` or `TRY` laid out by `layout` run: a
/// scrutinee that is `Never` takes none. Else each arm's verdict: *never* where the scrutinee can
/// never satisfy its guard, *always* where its upper end fits a guard that may always admit, or
/// the guard is `_` under `WITH`, and *maybe* otherwise. An arm an *always* guarded one outranks
/// never runs where every pair ranks, nor does `_` beside an *always* guarded one; a `MATCH` none
/// is left to is refused. Where every arm left always admits, the one they rank first is chosen,
/// and several ranked alike are refused.
fn armed(layout: &ArmsLayout, guard: impl Fn(u16) -> GuardAsks, asks: &mut dyn Asks) -> Chosen {
    let yes = |asks: &mut dyn Asks, at: u16| asks.ask(at) == Some(Answer::Bool(true));
    if (0..layout.arriving).any(|at| yes(asks, at)) {
        return Chosen::Outcome(SiteAnswer::Verdict(Verdict::Never));
    }
    let count = layout.count;
    let (mut never, mut always) = (0u64, 0u64);
    for at in 0..count {
        let each = guard(at);
        match (each.never, each.always) {
            (Some(ask), _) if asks.ask(ask) == Some(Answer::Never) => never |= 1 << at,
            (_, Some(ask)) if yes(asks, ask) => always |= 1 << at,
            (None, None) if each.guard == GuardKind::Default => always |= 1 << at,
            _ => {}
        }
    }
    // Which arms each strictly outranks, a bit each: a value outranks every type, and a type
    // another it lies strictly under. Open where an ask leaves a pair unranked.
    let mut beats = [0u64; 64];
    let mut closed = true;
    for a in 0..count {
        for b in (0..count).filter(|b| *b != a) {
            let (x, y) = (guard(a), guard(b));
            let outranks = match (x.rank, y.rank) {
                (Some(ra), Some(rb)) => match asks.ask(layout.under(ra, rb)) {
                    Some(Answer::Bool(false)) => Some(false),
                    Some(Answer::Bool(true)) => match asks.ask(layout.under(rb, ra)) {
                        Some(Answer::Bool(under)) => Some(!under),
                        _ => None,
                    },
                    _ => None,
                },
                _ => Some(x.guard == GuardKind::Value && y.guard == GuardKind::Type),
            };
            match outranks {
                Some(true) => beats[usize::from(a)] |= 1 << b,
                Some(false) => {}
                None => closed = false,
            }
        }
    }
    let guarded = (0..count)
        .filter(|at| always & 1 << at != 0 && guard(*at).guard != GuardKind::Default)
        .fold(0u64, |mask, at| mask | 1 << at);
    let mut kept = 0u64;
    for at in (0..count).filter(|at| never & 1 << at == 0) {
        let maybe = always & 1 << at == 0;
        let default = guard(at).guard == GuardKind::Default;
        let outranked = (0..count).any(|by| {
            guarded & 1 << by != 0
                && (default || (maybe && closed && beats[usize::from(by)] & 1 << at != 0))
        });
        if !outranked {
            kept |= 1 << at;
        }
    }
    if kept == 0 {
        return Chosen::Outcome(SiteAnswer::Refused(layout.no_admitting()));
    }
    let every = kept & !always == 0;
    let chosen = match kept.count_ones() {
        1 => every.then(|| kept.trailing_zeros()),
        _ if every && closed => {
            let beaten = (0..count)
                .filter(|at| kept & 1 << at != 0)
                .fold(0u64, |mask, at| mask | beats[usize::from(at)]);
            let first = kept & !beaten;
            match first.count_ones() {
                1 => Some(first.trailing_zeros()),
                tied => return Chosen::Outcome(SiteAnswer::Refused(layout.ambiguous(tied))),
            }
        }
        _ => None,
    };
    Chosen::Kept {
        kept,
        always: always & kept,
        ranked: closed,
        chosen,
    }
}

/// What a use's selection chooses before any candidate's points are read, or which arms of a
/// `MATCH` run ([`armed`]): an outcome no candidate's points change, or the candidates or arms
/// kept beside the one every binding that agrees on the answers runs.
enum Chosen {
    /// `Never`, undecided or refused.
    Outcome(SiteAnswer),
    Kept {
        kept: u64,
        always: u64,
        ranked: bool,
        chosen: Option<u32>,
    },
}

/// The walk's own selection over `count` candidates laid out by `layout`: an argument that is
/// `Never` never runs the use. Else each candidate's verdict ([`admits`]): a use none admits is
/// refused in the words the walk refuses it in; past that, an argument a solve reads that is
/// `Any` leaves the use to the call. A *maybe* one an *always* one outranks at the first class
/// never runs. Where every one left always admits, the one the table ranks first is chosen, and
/// several ranked alike are refused.
fn chosen<'c>(
    layout: &Selecting,
    count: u32,
    candidate: impl Fn(u32) -> &'c Selectable<'c> + Copy,
    asks: &mut dyn Asks,
) -> Chosen {
    let yes = |asks: &mut dyn Asks, at: u16| asks.ask(at) == Some(Answer::Bool(true));
    let (arriving, open) = (layout.arriving, layout.arriving + layout.open);
    if (0..arriving).any(|at| yes(asks, at)) {
        return Chosen::Outcome(SiteAnswer::Verdict(Verdict::Never));
    }
    let left = (arriving..open).any(|at| yes(asks, at));
    let (mut judged, mut always) = (0u64, 0u64);
    // The last instance argument's refusal a candidate was dropped over, and the first need that
    // dropped one, by its position on the list, as the walk words a use left with none.
    let (mut refused, mut dropped) = (layout.refused, layout.dropped);
    for at in 0..count {
        let admission = admits(candidate(at), asks);
        if let Some(refusal) = admission.refused {
            refused = Some(match layout.listed {
                1 => refusal,
                _ => layout.no_instance_at(),
            });
        }
        let position = candidate(at).position;
        if let Some(lacks) = admission.lacks
            && dropped.is_none_or(|(first, _)| position < first)
        {
            dropped = Some((position, lacks));
        }
        match admission.verdict {
            None | Some(Verdict::Never) => {}
            Some(verdict) => {
                judged |= 1 << at;
                if verdict == Verdict::Always {
                    always |= 1 << at;
                }
            }
        }
    }
    if judged == 0 {
        let lacks = dropped.and_then(|(_, lacks)| lacks);
        let refusal = refused.or(lacks).unwrap_or(layout.no_admitting());
        return Chosen::Outcome(SiteAnswer::Refused(refusal));
    }
    // An argument a solve reads that is `Any` leaves the selection to the call, past the
    // candidates whose instance slot it leaves unfixed.
    if left {
        return Chosen::Outcome(SiteAnswer::Undecided);
    }
    let outranks = |asks: &mut dyn Asks, class: u16, a: u32, b: u32| match (
        candidate(a).rank,
        candidate(b).rank,
    ) {
        (Some(a), Some(b)) => asks.ask(layout.outranks(class, a, b)),
        _ => None,
    };
    let each = |mask: u64| (0..count).filter(move |at| mask & 1 << at != 0);
    for maybe in each(judged & !always) {
        if each(always).any(|by| outranks(asks, 0, by, maybe) == Some(Answer::Bool(true))) {
            judged &= !(1 << maybe);
        }
    }
    let ranks = each(judged).all(|at| candidate(at).one && candidate(at).rank.is_some());
    let mut ranked = ranks && judged.count_ones() > 1;
    'pairs: for class in (0..layout.classes).take_while(|_| ranked) {
        for (a, b) in each(judged).flat_map(|a| each(judged).map(move |b| (a, b))) {
            if a != b && outranks(asks, class, a, b).is_none() {
                ranked = false;
                break 'pairs;
            }
        }
    }
    let mut chosen = None;
    if always == judged {
        chosen = match judged.count_ones() {
            1 => Some(judged.trailing_zeros()),
            _ if ranked => {
                let mut survivors = judged;
                for class in 0..layout.classes {
                    if survivors.count_ones() <= 1 {
                        break;
                    }
                    let mut beaten = 0u64;
                    for (a, b) in each(survivors).flat_map(|a| each(survivors).map(move |b| (a, b)))
                    {
                        if a != b && outranks(asks, class, a, b) == Some(Answer::Bool(true)) {
                            beaten |= 1 << b;
                        }
                    }
                    survivors &= !beaten;
                }
                let builtins = each(survivors).find(|at| candidate(*at).builtin);
                match survivors.count_ones() {
                    1 => Some(survivors.trailing_zeros()),
                    tied => match builtins {
                        Some(builtin) => Some(builtin),
                        None => {
                            return Chosen::Outcome(SiteAnswer::Refused(layout.ambiguous(tied)));
                        }
                    },
                }
            }
            _ => None,
        };
    }
    // The candidates the text leaves a member argument to, each name declared `Any` read at its
    // ends, which must agree on its instance.
    if let Some(split) = layout.split {
        let unsplit = Selecting {
            split: None,
            ..*layout
        };
        let mut ended = Ended {
            asks,
            count,
            candidate,
        };
        if let Chosen::Kept {
            kept, chosen: None, ..
        } = self::chosen(&unsplit, count, candidate, &mut ended)
            && split_member(layout, count as u16, kept, &mut ended)
        {
            return Chosen::Outcome(SiteAnswer::Refused(split));
        }
    }
    Chosen::Kept {
        kept: judged,
        always,
        ranked,
        chosen,
    }
}

/// A selection's asks with each candidate's verdict asked with every name declared `Any` read at
/// its ends ([`Selectable::ended`]).
struct Ended<'q, F> {
    asks: &'q mut dyn Asks,
    count: u32,
    candidate: F,
}

impl<'c, F: Fn(u32) -> &'c Selectable<'c>> Asks for Ended<'_, F> {
    fn ask(&mut self, index: u16) -> Option<Answer> {
        let ended = (0..self.count).find_map(|at| {
            let each = (self.candidate)(at);
            each.ended.filter(|_| each.verdict == Some(index))
        });
        self.asks.ask(ended.unwrap_or(index))
    }
}

/// Whether the candidates `kept`, among `count`, want some member argument a binding finds
/// quantified at different types: no instance is the one every run of the use takes.
fn split_member(layout: &Selecting, count: u16, kept: u64, asks: &mut dyn Asks) -> bool {
    let no = |asks: &mut dyn Asks, at: u16| asks.ask(at) == Some(Answer::Bool(false));
    let each = || (0..count).filter(move |at| kept & 1 << at != 0);
    (0..layout.members).any(|member| {
        let mut pairs = each().flat_map(|a| each().filter(move |b| a < *b).map(move |b| (a, b)));
        kept.count_ones() > 1
            && no(asks, layout.plain(member, count))
            && pairs.any(|(a, b)| no(asks, layout.agree(member, count, a, b)))
    })
}

/// What a candidate admits ([`admits`]): its verdict, `None` where an instance argument's check
/// drops it, beside the arm of the refusal a lone candidate so dropped is refused in; and, where
/// its builtin's need drops it, the arm of what the argument lacks, where the need was narrowed by
/// a name.
struct Admission {
    verdict: Option<Verdict>,
    refused: Option<u16>,
    lacks: Option<Option<u16>>,
}

/// `each`'s verdict: `None` where an instance argument's slot is no one type or no instance lies
/// under it, else its verdict, *never* where a need drops it, at most *maybe* where a surfaced key
/// or a slot left undecided holds it.
fn admits(each: &Selectable<'_>, asks: &mut dyn Asks) -> Admission {
    let mut unfixed = false;
    for check in each.checks {
        match asks.ask(check.ask) {
            Some(Answer::Bool(answer)) if answer == check.when => {
                return Admission {
                    verdict: None,
                    refused: Some(check.refusal),
                    lacks: None,
                };
            }
            None if check.exact => unfixed = true,
            _ => {}
        }
    }
    let mut verdict = match each.verdict.and_then(|verdict| asks.ask(verdict)) {
        Some(Answer::Verdict(verdict)) => verdict,
        _ => Verdict::Maybe,
    };
    let mut lacks = None;
    if verdict != Verdict::Never
        && let Some(need) = each.needs.iter().find(|need| drops(asks, need))
    {
        verdict = Verdict::Never;
        let (from, to) = need.lacks;
        let mut named = each.lacks[usize::from(from)..usize::from(to)].iter();
        lacks = Some(
            named
                .find(|lack| {
                    lack.ask
                        .is_none_or(|ask| asks.ask(ask) == Some(Answer::Bool(true)))
                })
                .map(|lack| lack.refusal),
        );
    }
    if let Some(exact) = each.surfaced {
        verdict = match verdict {
            Verdict::Never if exact => Verdict::Never,
            _ => Verdict::Maybe,
        };
    }
    if unfixed && verdict == Verdict::Always {
        verdict = Verdict::Maybe;
    }
    Admission {
        verdict: Some(verdict),
        refused: None,
        lacks,
    }
}

/// A use's selection decision as a value: its asks, written over names, its layout and its
/// candidates, which the call's type and a content tree's call keep so a binding decides which
/// candidate runs ([`Selection::runs`]). The decision reads no point's upper end and no route.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selection<'a> {
    pub asks: &'a [Question<'a>],
    pub layout: Selecting,
    pub candidates: &'a [Selectable<'a>],
}

impl<'a> Selection<'a> {
    /// What the candidate at `at` does at its call ([`Run`]) as [`chosen`] decides it, read
    /// under every answer an undecided ask may give: the run every such completion agrees on,
    /// past those where the call is refused or never runs, which return no value; `None` where
    /// two disagree, or more completions than [`MOST_COMPLETIONS`] are read. The one chosen is
    /// selected and the rest never run; else those kept may run, a lone one retyped to its
    /// surfaced head selected; one left to the call may run any.
    pub fn runs(&self, at: u16, asks: &mut dyn Asks) -> Option<Run> {
        let candidates = self.candidates;
        let mut trail = Trail::default();
        let mut agreed = None;
        for _ in 0..MOST_COMPLETIONS {
            let mut completing = Completing {
                asks: &mut *asks,
                questions: self.asks,
                trail: &mut trail,
                read: 0,
            };
            let chosen = chosen(
                &self.layout,
                candidates.len() as u32,
                |at| &candidates[at as usize],
                &mut completing,
            );
            if completing.trail.over {
                return None;
            }
            let retyped = |completing: &mut Completing<'_, '_>| {
                let ask = candidates[usize::from(at)].retyped;
                ask.map(|ask| completing.ask(ask)) == Some(Some(Answer::Bool(true)))
            };
            let run = match chosen {
                Chosen::Outcome(SiteAnswer::Undecided) => Some(Run::May),
                Chosen::Outcome(_) => None,
                Chosen::Kept {
                    chosen: Some(chosen),
                    ..
                } => Some(match chosen == u32::from(at) {
                    true => Run::Selected,
                    false => Run::Never,
                }),
                Chosen::Kept { kept, .. } if kept & 1 << at == 0 => Some(Run::Never),
                Chosen::Kept { kept, .. } if kept == 1 << at && retyped(&mut completing) => {
                    Some(Run::Selected)
                }
                Chosen::Kept { .. } => Some(Run::May),
            };
            if completing.trail.over {
                return None;
            }
            match (agreed, run) {
                (_, None) => {}
                (None, Some(run)) => agreed = Some(run),
                (Some(agreed), Some(run)) if agreed != run => return None,
                (Some(_), Some(_)) => {}
            }
            if !trail.advance() {
                return Some(agreed.unwrap_or(Run::Never));
            }
        }
        None
    }

    /// [`runs`](Self::runs) with each ask answered rigidly: what the candidate at `at` does as
    /// every binding of the names the asks are written over agrees.
    pub fn rigid_runs(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        at: u16,
    ) -> Option<Run> {
        let asks = self.asks;
        let mut rigid = AskedBy(|index: u16| {
            rigid_answer(types, scratch, &|_| None, &asks[usize::from(index)])
        });
        self.runs(at, &mut rigid)
    }

    /// The candidate every binding of the names the asks are written over selects, by its index.
    pub fn selected(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Option<u16> {
        (0..self.candidates.len() as u16)
            .find(|at| self.rigid_runs(types, scratch, *at) == Some(Run::Selected))
    }

    /// This selection with its slices laid again through `lay`.
    pub fn laid<'to>(&self, lay: impl Lays<'to>) -> Selection<'to> {
        Selection {
            asks: collect(lay, self.asks.iter().map(|ask| ask.laid(lay))),
            layout: self.layout,
            candidates: collect(lay, self.candidates.iter().map(|each| each.laid(lay))),
        }
    }

    /// This selection with every type its asks name mapped by `declared`, laid through `lay`.
    pub fn mapped<'x>(&self, lay: impl Lays<'x>, declared: &mut Substitute<'_>) -> Selection<'x> {
        let asks = self.asks;
        Selection {
            asks: lay.fill(asks.len(), |at| asks[at].mapped(lay, declared)),
            layout: self.layout,
            candidates: collect(lay, self.candidates.iter().map(|each| each.laid(lay))),
        }
    }

    /// Feed `hasher` the selection's content, alike across two lays: every field the decision
    /// reads, then each ask.
    pub fn fed(&self, hasher: &mut DigestHasher) {
        let Selecting {
            listed,
            single,
            arriving,
            open,
            ranked,
            classes,
            ranks,
            refusals,
            refused,
            dropped,
            members,
            agrees,
            split,
        } = self.layout;
        for count in [
            listed,
            u16::from(single),
            arriving,
            open,
            ranked,
            classes,
            ranks,
            members,
            agrees,
        ] {
            hasher.count(usize::from(count));
        }
        let optional = |at: Option<u16>| at.map_or(0, |at| usize::from(at) + 1);
        hasher
            .count(usize::from(refusals))
            .count(optional(refused))
            .count(optional(dropped.map(|(position, _)| position)))
            .count(optional(dropped.and_then(|(_, lacks)| lacks)))
            .count(optional(split));
        hasher.count(self.candidates.len());
        for each in self.candidates {
            let bits = usize::from(each.one) | usize::from(each.builtin) << 1;
            hasher
                .count(usize::from(each.position))
                .count(bits)
                .count(optional(each.retyped))
                .count(optional(each.rank))
                .count(optional(each.verdict))
                .count(optional(each.ended))
                .count(each.surfaced.map_or(0, |exact| 1 + usize::from(exact)));
            hasher.count(each.checks.len());
            for check in each.checks {
                hasher
                    .count(usize::from(check.ask))
                    .count(usize::from(check.when))
                    .count(usize::from(check.exact))
                    .count(usize::from(check.refusal));
            }
            hasher.count(each.needs.len());
            for need in each.needs {
                hasher
                    .count(usize::from(need.outside))
                    .count(usize::from(need.lacks.0))
                    .count(usize::from(need.lacks.1));
                match need.narrowed {
                    None => _ = hasher.count(0),
                    Some((gate, answer, narrowed)) => {
                        hasher.count(1).count(usize::from(gate));
                        answer_fed(hasher, answer);
                        hasher.count(usize::from(narrowed));
                    }
                }
            }
            hasher.count(each.lacks.len());
            for lack in each.lacks {
                hasher
                    .count(optional(lack.ask))
                    .count(usize::from(lack.refusal));
            }
            hasher.count(optional(each.points.map(|points| points.len() as u16)));
            for point in each.points.unwrap_or_default() {
                hasher
                    .count(usize::from(point.exact))
                    .count(optional(point.carried));
            }
        }
        hasher.count(self.asks.len());
        for ask in self.asks {
            ask.fed(hasher);
        }
    }
}

/// Which arms of a `MATCH` or `TRY` run, as a value: its asks, written over names, its layout and
/// its arms, which the `MATCH`'s type keeps so a binding decides what each arm does
/// ([`ArmChoice::runs`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ArmChoice<'a> {
    pub asks: &'a [Question<'a>],
    pub layout: ArmsLayout,
    pub guards: &'a [GuardAsks],
}

impl<'a> ArmChoice<'a> {
    /// What the arm at `at` does ([`Run`]) as [`armed`] decides it, read under every answer an
    /// undecided ask may give: the run every such completion agrees on, past those where the
    /// `MATCH` is refused or its scrutinee never arrives, which return no value; `None` where two
    /// disagree, or more completions than [`MOST_COMPLETIONS`] are read. The one chosen is
    /// selected and the rest never run; else those kept may run.
    pub fn runs(&self, at: u16, asks: &mut dyn Asks) -> Option<Run> {
        let guards = self.guards;
        let mut trail = Trail::default();
        let mut agreed = None;
        for _ in 0..MOST_COMPLETIONS {
            let mut completing = Completing {
                asks: &mut *asks,
                questions: self.asks,
                trail: &mut trail,
                read: 0,
            };
            let chosen = armed(&self.layout, |at| guards[usize::from(at)], &mut completing);
            if completing.trail.over {
                return None;
            }
            let run = match chosen {
                Chosen::Outcome(_) => None,
                Chosen::Kept {
                    chosen: Some(chosen),
                    ..
                } => Some(match chosen == u32::from(at) {
                    true => Run::Selected,
                    false => Run::Never,
                }),
                Chosen::Kept { kept, .. } if kept & 1 << at == 0 => Some(Run::Never),
                Chosen::Kept { .. } => Some(Run::May),
            };
            match (agreed, run) {
                (_, None) => {}
                (None, Some(run)) => agreed = Some(run),
                (Some(agreed), Some(run)) if agreed != run => return None,
                (Some(_), Some(_)) => {}
            }
            if !trail.advance() {
                return Some(agreed.unwrap_or(Run::Never));
            }
        }
        None
    }

    /// [`runs`](Self::runs) with each ask answered rigidly: what the arm at `at` does as every
    /// binding of the names the asks are written over agrees.
    pub fn rigid_runs(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        at: u16,
    ) -> Option<Run> {
        let asks = self.asks;
        let mut rigid = AskedBy(|index: u16| {
            rigid_answer(types, scratch, &|_| None, &asks[usize::from(index)])
        });
        self.runs(at, &mut rigid)
    }

    /// This choice with its slices laid again through `lay`.
    pub fn laid<'to>(&self, lay: impl Lays<'to>) -> ArmChoice<'to> {
        ArmChoice {
            asks: collect(lay, self.asks.iter().map(|ask| ask.laid(lay))),
            layout: self.layout,
            guards: collect(lay, self.guards.iter().copied()),
        }
    }

    /// This choice with every type its asks name mapped by `declared`, laid through `lay`.
    pub fn mapped<'x>(&self, lay: impl Lays<'x>, declared: &mut Substitute<'_>) -> ArmChoice<'x> {
        let asks = self.asks;
        ArmChoice {
            asks: lay.fill(asks.len(), |at| asks[at].mapped(lay, declared)),
            layout: self.layout,
            guards: collect(lay, self.guards.iter().copied()),
        }
    }

    /// Feed `hasher` the choice's content, alike across two lays: its layout, each arm's asks,
    /// then each ask.
    pub fn fed(&self, hasher: &mut DigestHasher) {
        let ArmsLayout {
            count,
            arriving,
            ranks,
            ranked,
        } = self.layout;
        for count in [count, arriving, ranks, ranked] {
            hasher.count(usize::from(count));
        }
        let optional = |at: Option<u16>| at.map_or(0, |at| usize::from(at) + 1);
        for each in self.guards {
            hasher
                .count(each.guard as usize)
                .count(optional(each.never))
                .count(optional(each.always))
                .count(optional(each.rank));
        }
        hasher.count(self.asks.len());
        for ask in self.asks {
            ask.fed(hasher);
        }
    }
}

/// Asks answered by a function of their index.
pub struct AskedBy<F>(pub F);

impl<F: FnMut(u16) -> Option<Answer>> Asks for AskedBy<F> {
    fn ask(&mut self, index: u16) -> Option<Answer> {
        (self.0)(index)
    }
}

/// The most completions of a selection's undecided asks [`Selection::runs`] reads.
const MOST_COMPLETIONS: usize = 64;

/// The most undecided asks one completion answers.
const MOST_OPEN: usize = 8;

/// Where an enumeration of a decision's undecided asks stands: the answer each undecided ask read
/// takes, by the order the decision reads them, beside how many answers it has. A decision reads
/// its asks in an order its answers fix, so advancing the deepest answer first visits each
/// completion once.
#[derive(Default)]
struct Trail {
    taken: [u8; MOST_OPEN],
    counts: [u8; MOST_OPEN],
    /// The ask each depth answered, so a second read of it in one completion answers alike.
    asked: [u16; MOST_OPEN],
    len: usize,
    /// Whether a completion read more undecided asks than the trail holds.
    over: bool,
}

impl Trail {
    /// Move to the next completion: `false` where every one was visited.
    fn advance(&mut self) -> bool {
        while let Some(last) = self.len.checked_sub(1) {
            if self.taken[last] + 1 < self.counts[last] {
                self.taken[last] += 1;
                return true;
            }
            self.len = last;
        }
        false
    }
}

/// Asks answered as given, each undecided one by the answer the trail takes for it.
struct Completing<'q, 'a> {
    asks: &'a mut dyn Asks,
    questions: &'q [Question<'q>],
    trail: &'a mut Trail,
    /// How many undecided asks this completion has read.
    read: usize,
}

impl Asks for Completing<'_, '_> {
    fn ask(&mut self, index: u16) -> Option<Answer> {
        if let Some(answer) = self.asks.ask(index) {
            return Some(answer);
        }
        let trail = &mut *self.trail;
        let question = &self.questions[usize::from(index)];
        if let Some(depth) = (0..self.read).find(|depth| trail.asked[*depth] == index) {
            return Some(question.nth_answer(u32::from(trail.taken[depth])));
        }
        let depth = self.read;
        if depth == MOST_OPEN {
            trail.over = true;
            return None;
        }
        if depth == trail.len {
            trail.taken[depth] = 0;
            trail.counts[depth] = question.answer_count() as u8;
            trail.asked[depth] = index;
            trail.len += 1;
        }
        self.read += 1;
        Some(question.nth_answer(u32::from(trail.taken[depth])))
    }
}

/// Whether `need` drops its candidate: its argument's lower end lies outside it.
fn drops(asks: &mut dyn Asks, need: &Need) -> bool {
    let outside = match need.narrowed {
        Some((gate, answer, narrowed)) if asks.ask(gate) == Some(answer) => narrowed,
        _ => need.outside,
    };
    asks.ask(outside) == Some(Answer::Bool(true))
}
