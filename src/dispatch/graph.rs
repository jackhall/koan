//! Koan's **fact domain** over the binding graph: what a name is bound to, how a question is asked
//! of names bound to content, how a term over names is applied to a binding, and how a term
//! mentions the names it is over ([the binding graph](../../design/binding-graph.md),
//! [the crate's contract](../../bindgraph/README.md#the-domains-contract)).
//!
//! A name of the graph is one of the variables the types a question or a term names are written
//! over ([`NameVariable`]): a site name's lexical level, or a derived name (an answered name, or a
//! module binder's carried name). A module binder's **content name** is no variable: a question
//! reads it only through a view's carrier or a tree's identity over the binder.
//!
//! [`Content`] is what a binding gives a name: a type, the type a module carries, or a module's
//! digest beside its bare signature and sealing. A type may still be written over a caller's
//! names left open, each a [`DerivedTerm::Open`] named by its address, never by a lexical level:
//! two bodies number their names alike, so a level would relate a caller's open name to a
//! callee's own. An answered name no answer gives content is its [`DerivedTerm::Rigid`].
//!
//! - [`ask`] writes each content into the question's positions and asks the closed door, or the
//!   rigid answer where a position stays open. Each position whose content holds an open variable
//!   or a rigid is freshened apart from every other position, so no judgement relates two. A path
//!   class's variable it reads through module substitution over the class recipes the fold laid
//!   and the contents the binding gives the names a recipe reads, its content tree substituted
//!   under the binding ([`Modules`]).
//! - [`apply`] writes a binding into a term, each name it leaves open written as its open
//!   variable, and refuses where a derived name's rule cannot compute over the content.
//! - [`union`] and [`rigid`] are the crate's union of contents and an answered name's rigid.
//! - [`GraphDomain`] is the crate's domain over a base's [`GraphFacts`]: each of the above over
//!   the names the fold laid ([the fold](crate::build::lay)), which also classes each name a term
//!   mentions as the crate's growth check reads it.
//!
//! Every function here is closed over its inputs and types no statement; the walk is not reached.
//! So are [tree application](self::trees) and [module substitution](self::classes), over the laid facts and
//! a binding's content their traits hand them: the walk is one caller of each.

pub(super) mod classes;
mod modules;
#[cfg(test)]
mod tests;
pub(super) mod trees;

use std::cell::Cell;
use std::hash::{Hash, Hasher};

use bindgraph::{
    Answers, Binding, Bound, Domain, Doors, EdgeId, NameId, NodeId, QuestionId, Resting,
};

use self::classes::Classes;
pub use self::modules::Modules;
use self::modules::Substituting;

use crate::build::lay::{
    AskId, ClosedKey, GraphAnswers, GraphFacts, GraphKept, GraphTerm, Instances, NameKind,
    NameVariable, TermId,
};
use crate::memory::{BumpAllocator, BumpVec, resident};
use crate::scope::{
    Answer, Bare, Binds, CloseLaying, Closed, Closer, DerivedName, DerivedTerm, LoadFacts,
    NameAddress, Payload, Question, Resolved, Segment, ShapeError, SiteAnswer, Spellings, Unknown,
    answer, free_variables, rebound, rigid_answer,
};
use crate::symbols::SymbolInterner;
use crate::type_lattice::{
    DeclaredType, DerivedId, Interval, KType, Parametric, PathClass, Side, Variable, read_through,
};
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{Chain, ContentDigest};

/// What a binding gives one name of the graph.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Content<'c> {
    /// A type: closed, or written over a caller's names left open, each its open variable.
    Type(DeclaredType<Parametric>),
    /// The type a module binder's module carries: its carried name's content.
    Carried(KType),
    /// A module binder's content name's: its module's value digest, beside its bare signature and
    /// sealing.
    Module {
        digest: ContentDigest,
        bare: Bare<'c>,
    },
}

impl Content<'_> {
    /// The content's digest: a type's handle digest, a carried type's, or a module's value digest
    /// folded with its bare signature's and sealing's handles. Never a region handle, which dies
    /// and is reused.
    pub fn digest(&self) -> u128 {
        let mut hasher = DigestHasher::new(Tag::Content);
        match *self {
            Content::Type(ty) => {
                hasher.count(0).feed(ty.digest());
            }
            Content::Carried(carried) => {
                hasher.count(1).feed(DeclaredType::Type(carried).digest());
            }
            Content::Module { digest, bare } => {
                hasher.count(2).digest(digest);
                hasher.feed(bare.signature.map(|ty| DeclaredType::Type(ty).digest()));
                if let Some(sealing) = bare.sealing {
                    let signature = DeclaredType::Type(sealing.underlying_signature).digest();
                    hasher.count(1).feed(signature).digest(sealing.underlying);
                    // A record's carriers are a function of `to`.
                    let record = sealing.record;
                    for ty in [record.application, record.from, record.to] {
                        hasher.feed(DeclaredType::Type(ty).digest());
                    }
                }
            }
        }
        hasher.finished().bits()
    }
}

impl Hash for Content<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u128(self.digest());
    }
}

/// One name a question or a term is over: the variable it is in their types, `None` for a module
/// binder's content name, its address and declared bound, and its content, `None` where open.
#[derive(Clone, Copy, Debug)]
pub struct Named<'c> {
    pub variable: Option<NameVariable>,
    pub address: NameAddress,
    pub upper: KType,
    pub content: Option<Content<'c>>,
}

/// The derived names the domain reads, and where it lays the names it mints: `names` finds a
/// name's term by its id, and a minted name is named through `spellings` and handed to `laid`.
#[derive(Clone, Copy)]
pub struct Naming<'n> {
    pub names: &'n dyn Fn(DerivedId) -> Option<DerivedName<'n>>,
    pub spellings: Spellings<'n>,
    pub laid: &'n dyn Fn(DerivedName<'_>),
}

impl Naming<'_> {
    /// The derived name of `term`: the one `names` holds, else named and laid.
    fn named(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        term: DerivedTerm<'_>,
    ) -> Parametric {
        if let Some(held) = (self.names)(term.id()) {
            return held.ty;
        }
        let name = self.spellings.named((types, scratch), term);
        (self.laid)(name);
        name.ty
    }

    /// Whether the derived name `id` is a rigid or an open variable.
    fn unknown(&self, id: DerivedId) -> bool {
        (self.names)(id).is_some_and(|name| {
            matches!(
                name.term,
                DerivedTerm::Rigid { .. } | DerivedTerm::Open { .. }
            )
        })
    }
}

/// A binding written in by name variable: each beside the type written for it, beside every name
/// the binding gives content, which module substitution reads.
struct Writes<'w, 'n, 'g> {
    written: &'w [(NameVariable, Parametric)],
    named: &'w [Named<'g>],
    modules: Option<Modules<'w, 'g>>,
    naming: Naming<'n>,
}

impl Writes<'_, '_, '_> {
    fn find(&self, variable: NameVariable) -> Option<Parametric> {
        (self.written.iter()).find_map(|(held, ty)| (*held == variable).then_some(*ty))
    }

    /// The type the binding gives `unknown`: a type name's closed type, or the type a module
    /// binder's module carries.
    fn given(&self, types: &Chain<'_, '_, '_, '_>, unknown: Unknown) -> Option<KType> {
        let address = NameAddress::Site(unknown);
        (self.named.iter())
            .filter(|named| named.address == address)
            .find_map(|named| match named.content? {
                Content::Carried(carried) => Some(carried),
                Content::Type(DeclaredType::Type(ty)) => types.concrete(ty),
                Content::Type(DeclaredType::Scheme(_)) | Content::Module { .. } => None,
            })
    }
}

impl Binds for Writes<'_, '_, '_> {
    fn level(&self, types: &Chain<'_, '_, '_, '_>, level: usize) -> Option<KType> {
        types.concrete(self.find(NameVariable::Level(level))?)
    }

    fn term(&self, id: DerivedId) -> Option<DerivedTerm<'_>> {
        (self.naming.names)(id).map(|name| name.term)
    }

    /// Module substitution over the laid recipes and the binding ([`Classes::class_bound`]).
    fn class(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        class: PathClass,
    ) -> Option<Resolved> {
        let modules = self.modules?;
        let facts = Substituting {
            writes: self,
            modules,
            types,
            scratch,
            recipe: Cell::new(None),
            applied: Cell::new(None),
        };
        let classes = Classes {
            types,
            scratch,
            shapes: modules.load.shapes(),
            facts: &facts,
        };
        classes.class_bound(modules.segment, class)
    }

    fn carried(&self, binder: Unknown) -> Option<KType> {
        let address = NameAddress::Site(binder);
        (self.named.iter())
            .filter(|named| named.address == address)
            .find_map(|named| match named.content? {
                Content::Carried(carried) => Some(carried),
                _ => None,
            })
    }

    fn name(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        term: DerivedTerm<'_>,
    ) -> Option<Parametric> {
        Some(self.naming.named(types, scratch, term))
    }

    fn written(&self, _: &Chain<'_, '_, '_, '_>, variable: Variable) -> Option<Parametric> {
        self.find(NameVariable::of(variable)?)
    }
}

/// The type `content` writes for a name, where it writes one: a module's content writes none.
fn content_type(content: Content<'_>) -> Option<Parametric> {
    match content {
        Content::Type(DeclaredType::Type(ty)) => Some(ty),
        Content::Carried(carried) => Some(carried.into()),
        // A scheme names no variable a read through intervals writes, and a module's content
        // is read through its views alone.
        Content::Type(DeclaredType::Scheme(_)) | Content::Module { .. } => None,
    }
}

/// `ty` with every rigid and open variable it holds freshened to the open variable of the ask's
/// `position`, so no judgement relates it to another position's.
fn freshened(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    naming: Naming<'_>,
    ty: Parametric,
    position: usize,
) -> Parametric {
    if types.concrete(ty).is_some() {
        return ty;
    }
    read_through(types, scratch, ty, Side::Above, &mut |variable| {
        let Variable::Derived { term, bound, .. } = variable else {
            return None;
        };
        if !naming.unknown(term) {
            return None;
        }
        let name = NameAddress::Asked {
            position: position as u16,
            of: term,
        };
        let open = DerivedTerm::Open { name, upper: bound };
        Some(Interval::point(naming.named(types, scratch, open)))
    })
}

/// **Asks** `question`, written over `names`, of the content each holds: each content written into
/// the question's positions, each position whose content holds a rigid or an open variable
/// freshened apart, then the doors' own rigid answer ([`rigid_answer`]): the closed door where
/// every type is closed, else the answer every binding gives. `None` is **undecided**: the
/// question's rigid answer is none.
pub fn ask<'g>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    naming: Naming<'_>,
    modules: Option<Modules<'_, 'g>>,
    question: &Question<'_>,
    names: &[Named<'g>],
) -> Option<Answer> {
    let mut written = BumpVec::with_capacity_in(names.len(), scratch);
    for (position, named) in names.iter().enumerate() {
        let (Some(variable), Some(content)) = (named.variable, named.content) else {
            continue;
        };
        if let Some(ty) = content_type(content) {
            written.push((variable, freshened(types, scratch, naming, ty, position)));
        }
    }
    let writes = Writes {
        written: &written,
        named: names,
        modules,
        naming,
    };
    let (asked, _) = question.substituted(types, scratch, &writes);
    let terms = |id| (naming.names)(id).map(|name| name.term);
    rigid_answer(types, scratch, &terms, &asked).or_else(|| match asked {
        // A verdict, a candidate's run and an arm's run hold an answer, *maybe* or *may*, that
        // stands for every binding of the names left open.
        Question::Verdict { .. } | Question::Runs { .. } | Question::Arms { .. } => {
            answer(types, scratch, &asked)
        }
        _ => None,
    })
}

/// `term`, written over `names`, under the binding they hold: each name's content written in, each
/// name left open written as its open variable, and each derived name the term reads handed its
/// term again. `None` where a derived name's rule computes nothing over the content, so the term
/// still reads a name the binding binds.
pub fn apply<'g>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    naming: Naming<'_>,
    term: DeclaredType<Parametric>,
    names: &[Named<'g>],
) -> Option<DeclaredType<Parametric>> {
    let mut written = BumpVec::with_capacity_in(names.len(), scratch);
    for named in names {
        let Some(variable) = named.variable else {
            continue;
        };
        let ty = match named.content {
            Some(content) => content_type(content),
            None => {
                let open = DerivedTerm::Open {
                    name: named.address,
                    upper: named.upper,
                };
                Some(naming.named(types, scratch, open))
            }
        };
        if let Some(ty) = ty {
            written.push((variable, ty));
        }
    }
    let writes = Writes {
        written: &written,
        named: names,
        modules: None,
        naming,
    };
    let applied = match rebound(types, scratch, term, &writes) {
        crate::scope::Closed::Same => term,
        crate::scope::Closed::Type(closed) => closed.into(),
        crate::scope::Closed::Partial(partial) => partial,
    };
    let bound: BumpVec<'_, NameVariable> = {
        let mut bound = BumpVec::with_capacity_in(written.len(), scratch);
        bound.extend(written.iter().map(|(variable, _)| *variable));
        bound
    };
    (!reads_any(types, scratch, naming, applied, &bound)).then_some(applied)
}

/// Whether `ty` reads any of `names`, itself or through a derived name's term.
fn reads_any(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    naming: Naming<'_>,
    ty: DeclaredType<Parametric>,
    names: &[NameVariable],
) -> bool {
    (free_variables(types, scratch, ty).iter()).any(|variable| {
        if NameVariable::of(*variable).is_some_and(|name| names.contains(&name)) {
            return true;
        }
        let Variable::Derived { term, .. } = *variable else {
            return false;
        };
        (naming.names)(term).is_some_and(|name| {
            let mut reads = false;
            name.term.each_type(&mut |operand| {
                reads = reads || reads_any(types, scratch, naming, operand, names);
            });
            reads
        })
    })
}

/// The **union** of `contents`: the lattice union of their types. The union of none is `Never`,
/// which a union drops, so the empty union is no alternative by canonical shape. A module's
/// content is never an alternative, since no union term mentions a content name: a union of one
/// module's content is that content.
pub fn union<'c>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    contents: &[Content<'c>],
) -> Content<'c> {
    if let [only @ Content::Module { .. }] = contents {
        return *only;
    }
    let mut members = BumpVec::with_capacity_in(contents.len(), scratch);
    for content in contents {
        match content_type(*content) {
            Some(ty) => members.push(ty),
            None => panic!("a union is of types: {content:?}"),
        }
    }
    Content::Type(DeclaredType::Type(types.union_of(scratch, &members)))
}

/// The **rigid** of the name at `address`, under its declared type `upper`: a variable related to
/// no other.
pub fn rigid<'c>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    naming: Naming<'_>,
    address: NameAddress,
    upper: KType,
) -> Content<'c> {
    let term = DerivedTerm::Rigid {
        name: address,
        upper,
    };
    Content::Type(DeclaredType::Type(naming.named(types, scratch, term)))
}

/// The derived names a [`Naming`] mints while one `instance` call asks and applies, kept so a name
/// minted once is found again: a rigid or an open variable is freshened apart only where it is
/// known for one.
pub struct Minted<'x> {
    names: Cell<BumpVec<'x, DerivedName<'x>>>,
    scratch: BumpAllocator<'x>,
}

impl<'x> Minted<'x> {
    pub fn new(scratch: BumpAllocator<'x>) -> Self {
        Minted {
            names: Cell::new(BumpVec::new_in(scratch)),
            scratch,
        }
    }

    /// The minted name `id`.
    pub fn get(&self, id: DerivedId) -> Option<DerivedName<'x>> {
        let names = self.names.replace(BumpVec::new_in(self.scratch));
        let found = names.iter().find(|name| name.id == id).copied();
        self.names.set(names);
        found
    }

    /// Keeps `name`, laid in the scratch, unless it is kept already.
    pub fn keep(&self, name: DerivedName<'_>) {
        if self.get(name.id).is_none() {
            let mut names = self.names.replace(BumpVec::new_in(self.scratch));
            names.push(name.laid(self.scratch));
            self.names.set(names);
        }
    }
}

/// Koan's **domain** over one base's graph, for one `instance` call: the names its fold laid,
/// asked and applied through the load's chain and the names `naming` finds and mints.
pub struct GraphDomain<'d, 'g, 'tier, 'pair, 'reg, 'door> {
    pub facts: &'d GraphFacts<'g>,
    pub load: &'d LoadFacts<'g>,
    pub types: &'d Chain<'tier, 'pair, 'reg, 'door>,
    pub scratch: BumpAllocator<'d>,
    pub naming: Naming<'d>,
    /// Where the load notes each name an ask reads, beside the body it is asked in: what words a
    /// refusal under the binding of the instance answering it. `None` at a frame's instance.
    pub noted: Option<&'d Noted<'d>>,
}

/// The names the asks of the question being answered read, each beside the body it is asked in,
/// laid in the scratch as a list, the latest first.
pub struct Noted<'d> {
    scratch: BumpAllocator<'d>,
    latest: Cell<Option<&'d Note<'d>>>,
}

/// One noted name, the body its ask is asked in, and the name noted before it.
#[derive(Clone, Copy)]
struct Note<'d> {
    segment: Segment,
    named: Named<'d>,
    before: Option<&'d Note<'d>>,
}

impl<'d> Noted<'d> {
    fn new(scratch: BumpAllocator<'d>) -> Self {
        Noted {
            scratch,
            latest: Cell::new(None),
        }
    }

    /// Notes `names`, read by an ask in the body `segment`.
    fn note(&self, segment: Segment, names: &[Named<'d>]) {
        for named in names {
            let before = self.latest.get();
            let note = Note {
                segment,
                named: *named,
                before,
            };
            self.latest.set(Some(resident(self.scratch, note)));
        }
    }

    /// The names noted since the last take, the latest first, the notes left empty.
    fn taken(&self) -> impl Iterator<Item = (Segment, Named<'d>)> + 'd {
        let mut at = self.latest.take();
        std::iter::from_fn(move || {
            let note = at?;
            at = note.before;
            Some((note.segment, note.named))
        })
    }
}

impl<'d, 'g> GraphDomain<'d, 'g, '_, '_, '_, '_> {
    /// Each type `named` hands its visitor minted in the chain from the load's terms where the chain
    /// lacks it: a composition's graph names types its own walk interned, which a frame under it
    /// mints as it reads them. The program's load carries no terms, and every type it names rests
    /// in the frozen load.
    fn minting(&self, named: impl FnOnce(&mut dyn FnMut(DeclaredType<Parametric>))) {
        let terms = self.load.base_layer().terms();
        if terms.is_empty() {
            return;
        }
        named(&mut |ty| {
            if !self.types.resolves(ty)
                && let Some((_, term)) = terms.get(ty.digest())
            {
                self.types.mint(term);
            }
        });
    }

    /// The name `name` of the graph, the variable `variable` in what reads it, bound to `content`.
    fn named(
        &self,
        name: NameId,
        variable: Option<NameVariable>,
        content: Option<Content<'d>>,
    ) -> Named<'d> {
        let held = self.facts.name(name);
        Named {
            variable,
            address: held.address,
            upper: held.upper,
            content,
        }
    }

    /// `ty`, written over `variables`, with `binding` written in.
    fn applied(
        &self,
        ty: DeclaredType<Parametric>,
        variables: &[(NameId, NameVariable)],
        binding: &Binding<Self>,
    ) -> Option<DeclaredType<Parametric>> {
        let mut names = BumpVec::with_capacity_in(variables.len(), self.scratch);
        for (name, variable) in variables {
            names.push(self.named(*name, Some(*variable), binding.get(*name)));
        }
        apply(self.types, self.scratch, self.naming, ty, &names)
    }
}

impl<'d, 'g> Domain for GraphDomain<'d, 'g, '_, '_, '_, '_> {
    type Facts = GraphFacts<'g>;
    type Content = Content<'d>;

    fn ask(&self, question: AskId, contents: &[Option<Content<'d>>]) -> Option<Answer> {
        let asked = self.facts.ask(question);
        self.minting(|each| {
            asked.question.each_type(each);
            (asked.names.iter()).for_each(|name| each(self.facts.name(*name).upper.into()));
        });
        let mut names = BumpVec::with_capacity_in(asked.names.len(), self.scratch);
        for (at, variable) in asked.variables {
            let at = usize::from(*at);
            names.push(self.named(asked.names[at], Some(*variable), contents[at]));
        }
        // A name a class's recipe reads is in no variable.
        for (at, name) in asked.names.iter().enumerate() {
            if !(asked.variables.iter()).any(|(held, _)| usize::from(*held) == at) {
                names.push(self.named(*name, None, contents[at]));
            }
        }
        if let Some(noted) = self.noted {
            noted.note(asked.segment, &names);
        }
        let modules = Modules {
            facts: self.facts,
            load: self.load,
            segment: asked.segment,
        };
        ask(
            self.types,
            self.scratch,
            self.naming,
            Some(modules),
            &asked.question,
            &names,
        )
    }

    fn apply(&self, term: TermId, binding: &Binding<Self>) -> Option<Content<'d>> {
        self.minting(|each| match *self.facts.term(term) {
            GraphTerm::Name(name) => each(self.facts.name(name).upper.into()),
            GraphTerm::Type { ty, variables, .. } => {
                each(ty);
                (variables.iter()).for_each(|(name, _)| each(self.facts.name(*name).upper.into()));
            }
            GraphTerm::Carried {
                typed, variables, ..
            } => {
                each(typed.lower.into());
                each(typed.upper.into());
                (variables.iter()).for_each(|(name, _)| each(self.facts.name(*name).upper.into()));
            }
            GraphTerm::Module { .. } => {}
        });
        match *self.facts.term(term) {
            // A module binder's name left open passes nothing: only a type name is open.
            GraphTerm::Name(name) => binding.get(name).or_else(|| {
                let held = self.facts.name(name);
                if held.kind != NameKind::Type {
                    return None;
                }
                let open = DerivedTerm::Open {
                    name: held.address,
                    upper: held.upper,
                };
                let ty = self.naming.named(self.types, self.scratch, open);
                Some(Content::Type(DeclaredType::Type(ty)))
            }),
            // Where the ends close to one type, it is the content a frame binds; where they are one
            // type over open names, that type; else the binding fixes nothing.
            GraphTerm::Carried {
                typed, variables, ..
            } => {
                let lower = self.applied(typed.lower.into(), variables, binding)?;
                let upper = self.applied(typed.upper.into(), variables, binding)?;
                let closed = |end| match end {
                    DeclaredType::Type(ty) => self.types.concrete(ty),
                    DeclaredType::Scheme(_) => None,
                };
                match (closed(lower), closed(upper)) {
                    (Some(lower), Some(upper)) => {
                        (lower == upper).then_some(Content::Carried(upper))
                    }
                    _ => (lower == upper).then_some(Content::Type(upper)),
                }
            }
            GraphTerm::Module {
                content,
                segment,
                site,
            } => {
                let modules = Modules {
                    facts: self.facts,
                    load: self.load,
                    segment,
                };
                let bare = modules.bare(self.types, self.scratch, self.naming, content, site)?;
                Some(Content::Module {
                    digest: content,
                    bare,
                })
            }
            GraphTerm::Type { ty, variables, .. } => {
                self.applied(ty, variables, binding).map(Content::Type)
            }
        }
    }

    fn union(&self, alternatives: &[Content<'d>]) -> Content<'d> {
        union(self.types, self.scratch, alternatives)
    }

    fn rigid(&self, name: NameId) -> Content<'d> {
        let held = self.facts.name(name);
        self.minting(|each| each(held.upper.into()));
        rigid(
            self.types,
            self.scratch,
            self.naming,
            held.address,
            held.upper,
        )
    }

    fn digest(&self, content: Content<'d>) -> u128 {
        content.digest()
    }

    fn lasts(&self, content: Content<'d>) -> bool {
        lasts(self.types, self.load, content)
    }

    // A holder's name arrives with the content of its own kind that a run binds there: a closed
    // type, a carried signature, or a module. A rigid, or a union of several modules, is a type
    // standing for what the run binds, and arrives at no module-kind name.
    fn arrives(&self, name: NameId, content: Content<'d>) -> bool {
        match (self.facts.name(name).kind, content) {
            (NameKind::Type, Content::Type(DeclaredType::Type(ty))) => {
                self.types.concrete(ty).is_some()
            }
            (NameKind::Carried, Content::Carried(_))
            | (NameKind::Content, Content::Module { .. }) => true,
            _ => false,
        }
    }
}

/// Whether `content` lasts as long as the load: a type where it rests in the frozen load, and a
/// module's content where its tree is one the load computed.
fn lasts(types: &Chain<'_, '_, '_, '_>, load: &LoadFacts<'_>, content: Content<'_>) -> bool {
    match content {
        Content::Type(ty) => types.in_root(ty),
        Content::Carried(carried) => types.in_root(DeclaredType::Type(carried)),
        Content::Module { digest, .. } => load.tree_of(digest).is_some(),
    }
}

/// The answer function, noting each instance's refusal beside the call that entered that instance
/// and the names its asks read: of the refusals it answers as its own, the one a load walking its
/// text meets first ([`GraphFacts::refusal_order`]). Of the instances that refuse, the load reports
/// the first in the order a walk of the calls meets them ([`chosen`](Self::chosen)). A question in a
/// quote's code refuses nothing here: the `EVAL` running the code reports it, as its frame's
/// instance answers it.
struct Refusing<'f, 'g, 'd> {
    answers: GraphAnswers<'f, 'g>,
    shapes: &'g crate::scope::Shapes<'g>,
    noted: &'f Noted<'d>,
    /// Each evaluation the instance call enters, by its index: the call that entered it and the
    /// evaluation that made that call.
    entries: BumpVec<'d, Option<(EdgeId, u32)>>,
    /// Each refusing evaluation's refusal, by its index.
    refused: BumpVec<'d, (u32, Refused<'d>)>,
    /// The first refusal answered as no evaluation's own: a borrowed question's, or a replay's.
    unowned: Option<Refused<'d>>,
}

/// A refused question, the arm of its template, the call that entered the instance answering it,
/// and the names its asks read there.
struct Refused<'d> {
    question: QuestionId,
    arm: u16,
    entered: Option<EdgeId>,
    names: Option<&'d Note<'d>>,
}

impl<'d> Refusing<'_, '_, 'd> {
    /// The refusal the load reports: of the evaluations that refuse, the first in a walk of the
    /// calls from the instance asked for, which meets an instance before the instances it calls
    /// and a caller's calls in the order its walk meets them ([`GraphFacts::call_order`]); else
    /// the first refusal no evaluation answers as its own.
    fn chosen(&self, scratch: BumpAllocator<'d>) -> Option<&Refused<'d>> {
        let facts = self.answers.facts;
        // The calls from the instance asked for down to `at`, outermost first.
        let path = |mut at: u32| {
            let mut path = BumpVec::new_in(scratch);
            while let Some(Some((call, from))) = self.entries.get(at as usize) {
                path.push(facts.call_order(*call));
                at = *from;
            }
            path.reverse();
            path
        };
        let paths = (self.refused.iter()).map(|(at, refused)| (path(*at), refused));
        let first = paths.min_by(|(a, _), (b, _)| a.as_slice().cmp(b.as_slice()));
        first.map(|(_, refused)| refused).or(self.unowned.as_ref())
    }
}

impl<'g> Answers<GraphFacts<'g>> for Refusing<'_, 'g, '_> {
    fn answer(
        &mut self,
        question: QuestionId,
        doors: &mut Doors<'_, GraphFacts<'g>>,
    ) -> SiteAnswer {
        self.noted.latest.set(None);
        let answered = self.answers.answer(question, doors);
        let facts = self.answers.facts;
        let in_code = || {
            (facts.sites.iter())
                .any(|((segment, ..), id)| *id == question && self.shapes.in_code(*segment))
        };
        if let SiteAnswer::Refused(arm) = answered
            && !in_code()
        {
            let refusal = Refused {
                question,
                arm,
                entered: doors.entered(),
                names: self.noted.latest.take(),
            };
            let order = |refused: &Refused<'_>| {
                facts.refusal_order(self.shapes, refused.question, refused.arm)
            };
            match doors.own_evaluation() {
                Some(own) => match self.refused.iter_mut().find(|(at, _)| *at == own) {
                    Some((_, held)) if order(&refusal) < order(held) => *held = refusal,
                    Some(_) => {}
                    None => self.refused.push((own, refusal)),
                },
                None => {
                    self.unowned.get_or_insert(refusal);
                }
            }
        }
        answered
    }

    fn entering(&mut self, evaluation: u32, by: Option<(EdgeId, u32)>) {
        debug_assert_eq!(
            evaluation as usize,
            self.entries.len(),
            "evaluations enter in order"
        );
        self.entries.push(by);
    }
}

/// A closer writing in what the names a refused question's asks read are bound to: the load's
/// wording of a refusal under the binding of the instance answering it, as a frame's binding words
/// it at the run. A type the chain does not resolve is left as it is.
struct Wording<'w, 'n, 'g, 'c, 'tier, 'pair, 'reg, 'door, 'x> {
    types: &'c Chain<'tier, 'pair, 'reg, 'door>,
    scratch: BumpAllocator<'x>,
    writes: &'c Writes<'w, 'n, 'g>,
}

impl Closer for Wording<'_, '_, '_, '_, '_, '_, '_, '_, '_> {
    fn close(&mut self, ty: DeclaredType<Parametric>) -> Closed {
        if !self.types.resolves(ty) {
            return Closed::Same;
        }
        rebound(self.types, self.scratch, ty, self.writes)
    }

    fn concrete(&self, ty: Parametric) -> Option<KType> {
        self.types.concrete(ty)
    }

    fn rebuilt(
        &mut self,
        scheme: crate::type_lattice::Scheme,
        rebounds: crate::scope::Rebounds<'_>,
    ) -> Option<DeclaredType<KType>> {
        let (types, scratch) = (self.types, self.scratch);
        crate::scope::rebuilt_closed(types, scratch, scheme, rebounds, &mut |ty| self.close(ty))
    }
}

impl<'x> crate::scope::Laying<'x> for Wording<'_, '_, '_, '_, '_, '_, '_, '_, 'x> {
    type Lay = BumpAllocator<'x>;

    fn lay(&self) -> BumpAllocator<'x> {
        self.scratch
    }
}

/// `refusal`, a refused question's template, worded under `names`, what its asks read: each type
/// name's content written into its variable, and each module binder's read through the class
/// recipes the fold laid in the body they are asked in.
fn worded<'x>(
    (types, scratch, naming): (&Chain<'_, '_, '_, '_>, BumpAllocator<'x>, Naming<'_>),
    (facts, load): (&GraphFacts<'_>, &LoadFacts<'_>),
    names: &Noted<'_>,
    refusal: &'x ShapeError<'x>,
) -> &'x ShapeError<'x> {
    let mut segment = None;
    let mut named = BumpVec::new_in(scratch);
    for (asked, each) in names.taken() {
        segment = Some(asked);
        named.push(each);
    }
    let Some(segment) = segment else {
        return refusal;
    };
    let mut written = BumpVec::with_capacity_in(named.len(), scratch);
    written.extend(
        (named.iter()).filter_map(|named| Some((named.variable?, content_type(named.content?)?))),
    );
    let writes = Writes {
        written: &written,
        named: &named,
        modules: Some(Modules {
            facts,
            load,
            segment,
        }),
        naming,
    };
    refusal.closed_laying(&mut Wording {
        types,
        scratch,
        writes: &writes,
    })
}

/// The derived name `name` of `load`, each type it and its term read minted in `types` from the
/// load's terms where the chain lacks it: a composition's names read types its own walk interned.
fn minted_in<'a>(
    types: &Chain<'_, '_, '_, '_>,
    load: &LoadFacts<'_>,
    name: DerivedName<'a>,
) -> DerivedName<'a> {
    let terms = load.base_layer().terms();
    if terms.is_empty() {
        return name;
    }
    let mut mint = |ty: DeclaredType<Parametric>| {
        if !types.resolves(ty)
            && let Some((_, term)) = terms.get(ty.digest())
        {
            types.mint(term);
        }
    };
    mint(name.ty.into());
    name.term.each_type(&mut mint);
    name
}

/// Makes the instance of `node` of `instances`' graph under the binding `bound` gives its names,
/// read off a frame's slots, and writes its handle into `into`: found filed among `instances`, or
/// made. The first question the node owns that the instance refuses, by its arm. A type name
/// takes a bound type; a module binder's carried name the type its module carries, and its
/// content name the module's digest beside its bare signature and sealing.
pub(super) fn instance<'graph>(
    instances: &Instances<'graph>,
    node: NodeId,
    bound: &[(NameId, crate::scope::Bound<'_>)],
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    into: &mut [u64],
) -> Option<(QuestionId, u16)> {
    let (facts, load) = (instances.binding.facts, instances.load);
    let minted = Minted::new(scratch);
    let names = |id| {
        load.derived_name(id)
            .map(|name| minted_in(types, load, name))
            .or_else(|| minted.get(id))
    };
    let keep = |name: DerivedName<'_>| minted.keep(name);
    let naming = Naming {
        names: &names,
        spellings: instances.spellings,
        laid: &keep,
    };
    let domain = GraphDomain {
        facts,
        load,
        types,
        scratch,
        naming,
        noted: None,
    };
    let mut binding = Binding::new();
    for (name, held) in bound.iter().copied() {
        let content = match (facts.name(name).kind, held) {
            (NameKind::Type, crate::scope::Bound::Type(ty)) => {
                Content::Type(DeclaredType::Type(Parametric::from(ty)))
            }
            (
                NameKind::Carried,
                crate::scope::Bound::Module { carried, .. } | crate::scope::Bound::Carried(carried),
            ) => Content::Carried(carried),
            // A module seen through a header is its bare member's content.
            (NameKind::Content, crate::scope::Bound::Module { tree, .. }) => {
                let bare = match *tree {
                    crate::scope::Tree::Retyped { source, .. } => source,
                    _ => tree,
                };
                match *bare {
                    crate::scope::Tree::Digest(digest, Some(bare)) => Content::Module {
                        digest,
                        bare: *bare,
                    },
                    _ => continue,
                }
            }
            _ => continue,
        };
        binding.bind(name, content);
    }
    instances.instance(node, &binding, &domain, into)
}

/// `ty`, a type a base record names, read in the body `segment`, with `binding` written in: each
/// type name's closed type at its level, and each module binder's carried type and module, which
/// module substitution reads through the class recipes the fold laid. A derived name is the
/// load's; a name a rule mints over the binding is kept in `scratch`.
pub(super) fn rebound_under(
    instances: &Instances<'_>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    segment: crate::scope::Segment,
    binding: crate::scope::Binding<'_>,
    ty: DeclaredType<Parametric>,
) -> crate::scope::Closed {
    let load = instances.load;
    let shapes = load.shapes();
    let minted = Minted::new(scratch);
    let found = |id| {
        load.derived_name(id)
            .map(|name| minted_in(types, load, name))
            .or_else(|| minted.get(id))
    };
    let keep = |name: DerivedName<'_>| minted.keep(name);
    let naming = Naming {
        names: &found,
        spellings: instances.spellings,
        laid: &keep,
    };
    let mut named = BumpVec::with_capacity_in(binding.entries().len(), scratch);
    let mut written = BumpVec::with_capacity_in(binding.entries().len(), scratch);
    for (unknown, bound) in binding.entries().iter().copied() {
        let address = NameAddress::Site(unknown);
        let name = |variable, content| Named {
            variable,
            address,
            upper: KType::ANY,
            content: Some(content),
        };
        match bound {
            crate::scope::Bound::Type(closed) => {
                let level = shapes.level(unknown.segment, unknown.at);
                let variable = level.map(NameVariable::Level);
                named.push(name(
                    variable,
                    Content::Type(DeclaredType::Type(closed.into())),
                ));
                if let Some(variable) = variable {
                    written.push((variable, Parametric::from(closed)));
                }
            }
            crate::scope::Bound::Carried(carried) => {
                named.push(name(None, Content::Carried(carried)));
            }
            crate::scope::Bound::Module { carried, tree } => {
                named.push(name(None, Content::Carried(carried)));
                // A module seen through a header is its bare member's content.
                let bare = match *tree {
                    crate::scope::Tree::Retyped { source, .. } => source,
                    _ => tree,
                };
                if let crate::scope::Tree::Digest(digest, Some(bare)) = *bare {
                    named.push(name(
                        None,
                        Content::Module {
                            digest,
                            bare: *bare,
                        },
                    ));
                }
            }
        }
    }
    // What the binding closes a type to is a function of the body, the type and what the binding
    // binds: where all three last, it is kept, as a view is, for a later frame to read back.
    let lasting = types.in_root(ty)
        && (named.iter()).all(|named| {
            named
                .content
                .is_none_or(|content| lasts(types, load, content))
        });
    let mut entries = BumpVec::with_capacity_in(binding.entries().len(), scratch);
    let keyed = lasting
        && binding.entries().iter().all(|(unknown, bound)| {
            let key = match *bound {
                crate::scope::Bound::Type(closed) | crate::scope::Bound::Carried(closed) => {
                    Some((*unknown, closed, None))
                }
                crate::scope::Bound::Module { carried, tree } => tree
                    .digest()
                    .map(|digest| (*unknown, carried, Some(digest))),
            };
            key.map(|key| entries.push(key)).is_some()
        });
    let key = keyed.then(|| ClosedKey {
        segment,
        ty,
        binding: &entries,
    });
    if let Some(closed) = key.and_then(|key| instances.kept_closed(key)) {
        return closed;
    }
    let writes = Writes {
        written: &written,
        named: &named,
        modules: Some(Modules {
            facts: instances.binding.facts,
            load,
            segment,
        }),
        naming,
    };
    let closed = rebound(types, scratch, ty, &writes);
    #[cfg(test)]
    CLOSED_FRESH.with(|count| count.set(count.get() + 1));
    let kept = match closed {
        crate::scope::Closed::Type(ty) => types.in_root(ty),
        crate::scope::Closed::Partial(ty) => types.in_root(ty),
        crate::scope::Closed::Same => true,
    };
    if let Some(key) = key.filter(|_| kept) {
        instances.keep_closed(key, closed);
    }
    closed
}

#[cfg(test)]
thread_local! {
    /// How many types a frame's binding closed afresh, finding none kept: what a test reads to see
    /// a binding close a type once. Tests reset it.
    pub(crate) static CLOSED_FRESH: Cell<usize> = const { Cell::new(0) };
}

/// Makes the root's instance of `bound`'x graph, binding nothing, and with it every instance it
/// reaches: the instances the load makes. The refusal the first refusing one makes ([`Refusing`]),
/// by its question and its template worded under the binding of that instance, beside the call
/// that entered it.
pub(super) fn made<'graph, 'x>(
    bound: &mut Bound<'_, '_, 'graph, GraphFacts<'graph>>,
    resting: &Resting<'graph, GraphKept>,
    facts: &'graph GraphFacts<'graph>,
    load: &'graph LoadFacts<'graph>,
    types: &Chain<'graph, 'graph, 'graph, '_>,
    scratch: BumpAllocator<'x>,
    symbols: &SymbolInterner,
) -> Option<(QuestionId, &'x ShapeError<'x>, Option<EdgeId>)>
where
    'graph: 'x,
{
    let graph = bound.graph();
    let minted = Minted::new(scratch);
    let names = |id| load.derived_name(id).or_else(|| minted.get(id));
    let keep = |name: DerivedName<'_>| minted.keep(name);
    let naming = Naming {
        names: &names,
        spellings: Spellings::new(symbols),
        laid: &keep,
    };
    let noted = &Noted::new(scratch);
    let domain = GraphDomain {
        facts,
        load,
        types,
        scratch,
        naming,
        noted: Some(noted),
    };
    let mut answers = Refusing {
        answers: GraphAnswers { facts },
        shapes: load.shapes(),
        noted,
        entries: BumpVec::new_in(scratch),
        refused: BumpVec::new_in(scratch),
        unowned: None,
    };
    let root = graph.root();
    let mut words = BumpVec::new_in(scratch);
    words.extend((0..graph.node(root).handle_width()).map(|_| 0u64));
    bound.instance(
        resting,
        root,
        &Binding::new(),
        &domain,
        &mut answers,
        &mut words,
    );
    let refused = answers.chosen(scratch)?;
    let stored = facts.question(refused.question).stored();
    let Some(Payload::Refusal(template)) =
        stored.map(|stored| stored.arms[usize::from(refused.arm)])
    else {
        panic!("a refused answer names its refusal's template");
    };
    noted.latest.set(refused.names);
    let refusal = worded((types, scratch, naming), (facts, load), noted, template);
    Some((refused.question, refusal, refused.entered))
}
