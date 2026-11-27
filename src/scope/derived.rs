//! **Derived names**: a type a typing rule needs written over run-bound names that no other lattice
//! node spells — a piece the bound does not guarantee, a meet, the result of a rule whose case a
//! binding decides — named by a [`TypeNode::Derived`] beside the **term** that types it
//! ([build/README.md § Derived names](../build/README.md#derived-names)).
//!
//! A derived name's term is one output of a [`Rule`] over operands written over names, given by the
//! rule's closed function, or a **decided** name: the question that decides it, beside one type per
//! answer ([`DerivedTerm::Decided`]). The binding graph's answered name, typed by one edge per arm
//! of its deciding question, is the same shape. Its closed ends hold every binding's. A `:Module`
//! binder is typed by the name of the type its argument carries ([`DerivedTerm::Carried`]), which
//! a binding of the binder gives.
//!
//! A name is identified by its term's content ([`DerivedTerm::id`]), as a path class is, so every
//! walk that meets a term names it alike. One function, [`handed`], gives a term its type — closed,
//! written over names, or the term's name — and a binding gives a derived name what `handed` gives
//! its term with the binding written in ([`rebound`]), so every reader writing one binding into a
//! name holds one handle. A walk lays each name it meets ([`Derived`]), and a reader finds a name's
//! term by its id ([`Binds::term`]).
//!
//! A derived name renders as its term: it is named through [`Spellings`], the symbols door, by the
//! spelling its rule gives (`(ELEMENT OF Elt)`). The lattice's `Term` is a type's minting term, a
//! different thing.

use crate::memory::{BumpAllocator, BumpVec, Lays, collect};
use crate::source::SourceRef;
use crate::symbols::{SymbolInterner, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, DerivedId, KType, Parametric, PathClass, Variable, bound_above, display_name,
    join,
};
use crate::values::Chain;
use crate::values::digest::{DigestHasher, Tag};

use super::facts::Unknown;
use super::nominal::{Handed, Nominals, sealed_nominals};
use super::question::{Question, free_variables, rebound};
use super::record_types::Closed;
use super::rigid::rigid_answer;
use super::rule::Rule;
use super::shape::Segment;
use super::signed::{Signed, sealed_signature};
use super::typed::Resolved;

/// How a [`DerivedTerm::Carried`] name is spelled: by the binder holding the module, or by the
/// call or `EVAL` written in place that a holder holds.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Carrier {
    Binder(ValueSymbol),
    Written(SourceRef),
}

/// What a derived name stands for, every type it reads written over names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DerivedTerm<'a> {
    /// Output `output` of `rule` over `operands`: a type no question decides, given by the rule's
    /// closed function.
    Output {
        rule: Rule,
        operands: &'a [DeclaredType<Parametric>],
        output: u8,
    },
    /// The type on the answer `decision` takes: one type per answer, so a binding gives the type
    /// on the answer the closed door gives it.
    Decided {
        decision: Decider<'a>,
        arms: &'a [Parametric],
    },
    /// Member `output` of the nominal component `group` sealed over `operands`, each type the
    /// declaration door read written over names ([`sealed_nominals`]): the identity a binding
    /// seals.
    Nominal {
        group: &'a Nominals<'a>,
        operands: &'a [Parametric],
        output: u8,
    },
    /// The declared signature `signed` reads at `operands`, each member's type written over names
    /// ([`sealed_signature`]): the signature a binding declares.
    Signature {
        signed: &'a Signed<'a>,
        operands: &'a [DeclaredType<Parametric>],
    },
    /// The type the module a binding binds to the module binder `binder`, spelled `name`,
    /// carries: a signature under `bound`, written over names, at every binding, which no other
    /// node spells. A `:Module` parameter's bound is the empty signature; a module binder the run
    /// builds lies under its static type's upper end.
    Carried {
        binder: Unknown,
        name: Carrier,
        bound: Parametric,
    },
    /// The rigid of the name at `name`: whatever the run binds there, where no answer gives the
    /// name content, under `upper`. One per name, related to no other variable, so two names
    /// numbered alike in two bodies never meet ([`NameAddress`]).
    Rigid { name: NameAddress, upper: KType },
    /// The name at `name` left open inside content a caller passes, or an ask's position whose
    /// content is open or a rigid, under `upper`: its own unknown, apart from every name the
    /// callee binds and every other position of the ask.
    Open { name: NameAddress, upper: KType },
}

/// Where a name a rigid or an open variable stands for lies in its base, stable across loads of
/// one text: a site name by its lexical address, an answered name by its deciding term's id, one
/// position of an ask whose content held the variable `of`, a callable's result by its body, or
/// what an answer through a call gives a holder, by the holder.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum NameAddress {
    Site(Unknown),
    Answered(DerivedId),
    Asked { position: u16, of: DerivedId },
    Returned(Segment),
    Arrival(Unknown),
}

impl DerivedTerm<'_> {
    /// Whether this is the type a module binder carries: a [`Carried`](DerivedTerm::Carried) name
    /// under a signature, so a signature at every binding. One under `Any`, a parameter declared
    /// `Any`, declares no bound.
    pub fn carries_a_module(&self) -> bool {
        matches!(*self, DerivedTerm::Carried { bound, .. }
            if bound != Parametric::from(KType::ANY))
    }
}

impl NameAddress {
    fn fed(&self, hasher: &mut DigestHasher) {
        match *self {
            NameAddress::Site(unknown) => {
                hasher.count(0).feed(unknown);
            }
            NameAddress::Answered(id) => {
                hasher.count(1).feed(id);
            }
            NameAddress::Asked { position, of } => {
                hasher.count(2).count(usize::from(position)).feed(of);
            }
            NameAddress::Returned(segment) => {
                hasher.count(3).feed(segment);
            }
            NameAddress::Arrival(unknown) => {
                hasher.count(4).feed(unknown);
            }
        }
    }
}

/// What decides a [decided](DerivedTerm::Decided) name's answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decider<'a> {
    /// A question, its answer's index the arm.
    Answer(Question<'a>),
}

/// What a binding gives the rigid names a type is written over: what a substitution reads.
pub trait Binds {
    /// The closed type the binding gives the lexical variable at `level`, where it binds it.
    fn level(&self, types: &Chain<'_, '_, '_, '_>, level: usize) -> Option<KType>;

    /// The term the derived name `id` names, where a table the reader reaches holds it. A name
    /// whose term no table holds is left as it is.
    fn term(&self, id: DerivedId) -> Option<DerivedTerm<'_>> {
        let _ = id;
        None
    }

    /// What the binding makes the variable of the path class `class`: the closed type a bound
    /// module binder the class's paths read through gives it (module substitution), or the class
    /// its paths fall in once the binding is written into their application or tree. `None` where
    /// it leaves the class as it is.
    fn class(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        class: PathClass,
    ) -> Option<Resolved> {
        let _ = (types, scratch, class);
        None
    }

    /// The derived name for `term`, a term no closed rule, spelling or rigid answer types, laid
    /// where the reader keeps the names it meets. `None` where the reader names none, which
    /// leaves the name it substitutes as it is.
    fn name(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        term: DerivedTerm<'_>,
    ) -> Option<Parametric> {
        let _ = (types, scratch, term);
        None
    }

    /// The type the module the binding binds to the module binder `binder` carries, where it binds
    /// one ([`DerivedTerm::Carried`]).
    fn carried(&self, binder: Unknown) -> Option<KType> {
        let _ = binder;
        None
    }

    /// What the binding writes in for `variable` whole, consulted before every other door: a type
    /// that may still be written over names, such as content holding an open variable. `None`
    /// where it leaves the other doors to answer.
    fn written(&self, types: &Chain<'_, '_, '_, '_>, variable: Variable) -> Option<Parametric> {
        let _ = (types, variable);
        None
    }
}

/// A binding of the levels `bound` gives, which names no derived name: a test's, or a closer's
/// over a quote's captures.
pub struct LevelsOnly<'f>(pub &'f dyn Fn(usize) -> Option<KType>);

impl Binds for LevelsOnly<'_> {
    fn level(&self, _: &Chain<'_, '_, '_, '_>, level: usize) -> Option<KType> {
        (self.0)(level)
    }
}

impl<'a> DerivedTerm<'a> {
    /// The term's name: a digest of its content, alike across two lays and every walk that meets
    /// it, as a path class is named by its content.
    pub fn id(&self) -> DerivedId {
        let mut hasher = DigestHasher::new(Tag::Derived);
        match *self {
            DerivedTerm::Output {
                rule,
                operands,
                output,
            } => {
                hasher.count(0).feed(rule).count(usize::from(output));
                hasher.count(operands.len());
                for operand in operands {
                    hasher.feed(operand.digest());
                }
            }
            DerivedTerm::Decided {
                decision: Decider::Answer(question),
                arms,
            } => {
                hasher.count(1);
                question.fed(&mut hasher);
                hasher.count(arms.len());
                for arm in arms {
                    hasher.feed(arm.digest());
                }
            }
            DerivedTerm::Nominal {
                group,
                operands,
                output,
            } => {
                hasher.count(2).feed(group).count(usize::from(output));
                hasher.count(operands.len());
                for operand in operands {
                    hasher.feed(operand.digest());
                }
            }
            DerivedTerm::Signature { signed, operands } => {
                hasher.count(3).feed(signed).count(operands.len());
                for operand in operands {
                    hasher.feed(operand.digest());
                }
            }
            DerivedTerm::Carried { binder, bound, .. } => {
                hasher.count(4).feed(binder).feed(bound.digest());
            }
            DerivedTerm::Rigid { name, upper } => {
                hasher.count(5);
                name.fed(&mut hasher);
                hasher.feed(upper.digest());
            }
            DerivedTerm::Open { name, upper } => {
                hasher.count(6);
                name.fed(&mut hasher);
                hasher.feed(upper.digest());
            }
        }
        let bits = hasher.finished().bits();
        DerivedId([bits as u64, (bits >> 64) as u64])
    }

    /// Hand `each` every type the term reads: a rule's operands, or a decided name's question's
    /// types and its arms.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        match *self {
            DerivedTerm::Output { operands, .. } => operands.iter().for_each(|ty| each(*ty)),
            DerivedTerm::Decided {
                decision: Decider::Answer(question),
                arms,
            } => {
                question.each_type(each);
                arms.iter().for_each(|arm| each((*arm).into()));
            }
            DerivedTerm::Nominal { operands, .. } => {
                operands.iter().for_each(|operand| each((*operand).into()));
            }
            DerivedTerm::Signature { operands, .. } => operands.iter().for_each(|ty| each(*ty)),
            DerivedTerm::Carried { .. } => {}
            DerivedTerm::Rigid { upper, .. } | DerivedTerm::Open { upper, .. } => {
                each(upper.into());
            }
        }
    }

    /// The term with `binds` written into every type it reads, beside whether any changed.
    pub fn substituted<'x>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'x>,
        binds: &dyn Binds,
    ) -> (DerivedTerm<'x>, bool) {
        let mut changed = false;
        let mut written = |ty: DeclaredType<Parametric>| match rebound(types, scratch, ty, binds) {
            Closed::Same => ty,
            Closed::Type(closed) => {
                changed = true;
                closed.into()
            }
            Closed::Partial(partial) => {
                changed = true;
                partial
            }
        };
        let term = match *self {
            DerivedTerm::Output {
                rule,
                operands,
                output,
            } => {
                let mut each = BumpVec::with_capacity_in(operands.len(), scratch);
                each.extend(operands.iter().map(|operand| written(*operand)));
                DerivedTerm::Output {
                    rule,
                    operands: collect(scratch, each.iter().copied()),
                    output,
                }
            }
            DerivedTerm::Decided {
                decision: Decider::Answer(question),
                arms,
            } => {
                let mut each = BumpVec::with_capacity_in(arms.len(), scratch);
                each.extend(arms.iter().map(|arm| match written((*arm).into()) {
                    DeclaredType::Type(arm) => arm,
                    DeclaredType::Scheme(_) => unreachable!("an arm is a type"),
                }));
                let arms = collect(scratch, each.iter().copied());
                let (question, asked) = question.substituted(types, scratch, binds);
                changed |= asked;
                DerivedTerm::Decided {
                    decision: Decider::Answer(question),
                    arms,
                }
            }
            DerivedTerm::Nominal {
                group,
                operands,
                output,
            } => {
                let mut each = BumpVec::with_capacity_in(operands.len(), scratch);
                each.extend(
                    operands
                        .iter()
                        .map(|operand| match written((*operand).into()) {
                            DeclaredType::Type(operand) => operand,
                            DeclaredType::Scheme(_) => unreachable!("a schema reads a type"),
                        }),
                );
                DerivedTerm::Nominal {
                    group: crate::memory::resident(scratch, group.laid(scratch)),
                    operands: collect(scratch, each.iter().copied()),
                    output,
                }
            }
            DerivedTerm::Signature { signed, operands } => {
                let mut each = BumpVec::with_capacity_in(operands.len(), scratch);
                each.extend(operands.iter().map(|operand| written(*operand)));
                DerivedTerm::Signature {
                    signed: crate::memory::resident(scratch, signed.laid(scratch)),
                    operands: collect(scratch, each.iter().copied()),
                }
            }
            DerivedTerm::Carried {
                binder,
                name,
                bound,
            } => DerivedTerm::Carried {
                binder,
                name,
                bound: match written(bound.into()) {
                    DeclaredType::Type(bound) => bound,
                    DeclaredType::Scheme(_) => unreachable!("a bound is a type"),
                },
            },
            // A rigid's and an open variable's bound is closed: no binding writes into it.
            DerivedTerm::Rigid { name, upper } => DerivedTerm::Rigid { name, upper },
            DerivedTerm::Open { name, upper } => DerivedTerm::Open { name, upper },
        };
        (term, changed)
    }

    /// Closed ends holding what every binding gives the term: a rule's, or for a decided name
    /// `Never` up to the join of its arms' bounds. An end holding an opaque carrier is widened past
    /// it, a lower end to `Never` and an upper end to `Any`, since a name's bound holds no carrier
    /// (the hidden-bound law); the carrier stays in the term.
    pub fn ends(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> (KType, KType) {
        let (lower, bound) = self.carried_ends(types, scratch);
        (
            match types.holds_carrier(lower) {
                true => KType::NEVER,
                false => lower,
            },
            match types.holds_carrier(bound) {
                true => KType::ANY,
                false => bound,
            },
        )
    }

    /// [`ends`](Self::ends) before widening past a carrier.
    fn carried_ends(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> (KType, KType) {
        match *self {
            DerivedTerm::Output {
                rule,
                operands,
                output,
            } => rule.ends((types, scratch), operands, output),
            DerivedTerm::Decided { arms, .. } => {
                let bound = (arms.iter()).fold(KType::NEVER, |joined, arm| {
                    join(types, scratch, joined, bound_above(types, scratch, *arm))
                });
                (KType::NEVER, bound)
            }
            DerivedTerm::Nominal { .. } => (KType::NEVER, KType::ANY),
            // A signature lies under the empty signature at every binding.
            DerivedTerm::Signature { .. } => (KType::NEVER, KType::EMPTY_SIGNATURE),
            DerivedTerm::Carried { bound, .. } => {
                (KType::NEVER, bound_above(types, scratch, bound))
            }
            DerivedTerm::Rigid { upper, .. } | DerivedTerm::Open { upper, .. } => {
                (KType::NEVER, upper)
            }
        }
    }

    /// Write the spelling the term renders as: a rule's, or a decided name's arms, `(ONE OF a, b)`.
    fn spell(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        (types, symbols): (&Chain<'_, '_, '_, '_>, &SymbolInterner),
    ) -> std::fmt::Result {
        match *self {
            DerivedTerm::Output {
                rule,
                operands,
                output,
            } => rule.spell(f, (types, symbols), operands, output),
            DerivedTerm::Decided { arms, .. } => {
                f.write_str("(ONE OF ")?;
                for (at, arm) in arms.iter().enumerate() {
                    if at > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", display_name(*arm, types, symbols))?;
                }
                f.write_str(")")
            }
            DerivedTerm::Nominal {
                group,
                operands,
                output,
            } => {
                let named = match group.handles.get(usize::from(output)) {
                    Some(Handed::Member(index)) => group.members.get(*index).map(|member| member.0),
                    Some(Handed::Binder(name)) => Some(*name),
                    _ => None,
                };
                f.write_str("(")?;
                match named {
                    Some(name) => write!(f, "{}", symbols.display(name.symbol()))?,
                    None => write!(f, "#{output}")?,
                }
                f.write_str(" DECLARED OVER ")?;
                for (at, operand) in operands.iter().enumerate() {
                    if at > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", display_name(*operand, types, symbols))?;
                }
                f.write_str(")")
            }
            DerivedTerm::Signature { operands, .. } => {
                f.write_str("(SIG DECLARED OVER ")?;
                for (at, operand) in operands.iter().enumerate() {
                    if at > 0 {
                        f.write_str(", ")?;
                    }
                    match operand {
                        DeclaredType::Type(ty) => {
                            write!(f, "{}", display_name(*ty, types, symbols))?
                        }
                        DeclaredType::Scheme(_) => f.write_str("Any")?,
                    }
                }
                f.write_str(")")
            }
            DerivedTerm::Carried {
                name: Carrier::Binder(name),
                ..
            } => write!(f, "(CARRIED BY {})", symbols.display(name.symbol())),
            DerivedTerm::Carried {
                name: Carrier::Written(source),
                ..
            } => write!(f, "(CARRIED BY {})", source.text()),
            DerivedTerm::Rigid { name, .. } => {
                f.write_str("(RIGID ")?;
                spell_address(f, name)?;
                f.write_str(")")
            }
            DerivedTerm::Open { name, .. } => {
                f.write_str("(OPEN ")?;
                spell_address(f, name)?;
                f.write_str(")")
            }
        }
    }

    /// The term laid through `lay`.
    pub fn laid<'to>(&self, lay: impl Lays<'to>) -> DerivedTerm<'to> {
        match *self {
            DerivedTerm::Output {
                rule,
                operands,
                output,
            } => DerivedTerm::Output {
                rule,
                operands: collect(lay, operands.iter().copied()),
                output,
            },
            DerivedTerm::Decided {
                decision: Decider::Answer(question),
                arms,
            } => DerivedTerm::Decided {
                decision: Decider::Answer(question.laid(lay)),
                arms: collect(lay, arms.iter().copied()),
            },
            DerivedTerm::Nominal {
                group,
                operands,
                output,
            } => DerivedTerm::Nominal {
                group: crate::memory::resident(lay, group.laid(lay)),
                operands: collect(lay, operands.iter().copied()),
                output,
            },
            DerivedTerm::Signature { signed, operands } => DerivedTerm::Signature {
                signed: crate::memory::resident(lay, signed.laid(lay)),
                operands: collect(lay, operands.iter().copied()),
            },
            DerivedTerm::Carried {
                binder,
                name,
                bound,
            } => DerivedTerm::Carried {
                binder,
                name,
                bound,
            },
            DerivedTerm::Rigid { name, upper } => DerivedTerm::Rigid { name, upper },
            DerivedTerm::Open { name, upper } => DerivedTerm::Open { name, upper },
        }
    }
}

/// A name address as a rigid's or an open variable's spelling shows it: short, and unlike any
/// other address.
fn spell_address(f: &mut std::fmt::Formatter<'_>, name: NameAddress) -> std::fmt::Result {
    let short = |id: DerivedId| id.0[0] >> 40;
    match name {
        NameAddress::Site(unknown) => write!(f, "{}.{:?}", unknown.segment.index(), unknown.at),
        NameAddress::Answered(id) => write!(f, "{:x}", short(id)),
        NameAddress::Asked { position, of } => write!(f, "{:x}.{position}", short(of)),
        NameAddress::Returned(segment) => write!(f, "{}.returned", segment.index()),
        NameAddress::Arrival(unknown) => {
            write!(f, "{}.{:?}.arrival", unknown.segment.index(), unknown.at)
        }
    }
}

/// **The type `term` hands back**: a rule's closed output where every operand is closed, else what
/// the rule writes over the names, else `name`'s name for it; a decided term's one arm where every
/// arm is it, the arm of its rigid answer ([`rigid_answer`]), else `name`'s. A door hands back what
/// this gives its term, and a binding gives a derived name what this gives its term with the
/// binding written in, so a rule hands back one handle at load and under every binding. `None`
/// where the closed rule has no answer over closed operands; `name`'s error where it names none.
pub fn handed<E>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    term: DerivedTerm<'_>,
    name: &mut dyn FnMut(DerivedTerm<'_>) -> Result<Parametric, E>,
) -> Result<Option<Parametric>, E> {
    let named =
        |name: &mut dyn FnMut(DerivedTerm<'_>) -> Result<Parametric, E>| name(term).map(Some);
    match term {
        DerivedTerm::Output {
            rule,
            operands,
            output,
        } => {
            let mut closed = BumpVec::with_capacity_in(operands.len(), scratch);
            closed.extend(operands.iter().map_while(|ty| types.concrete_declared(*ty)));
            if closed.len() == operands.len() {
                let outputs = rule.closed(types, scratch, &closed);
                return Ok(outputs
                    .and_then(|outputs| outputs.get(usize::from(output)).map(|ty| (*ty).into())));
            }
            let spelled = rule.spelled(types, scratch, operands);
            match spelled.get(usize::from(output)).copied().flatten() {
                Some(written) => Ok(Some(written)),
                None => named(name),
            }
        }
        DerivedTerm::Decided {
            decision: Decider::Answer(question),
            arms,
        } => {
            if let [first, rest @ ..] = arms
                && rest.iter().all(|arm| arm == first)
            {
                return Ok(Some(*first));
            }
            // A decided name's question asks no module of a name, so it reads no term.
            match rigid_answer(types, scratch, &|_| None, &question) {
                Some(answer) => Ok(arms.get(answer.index_in(&question) as usize).copied()),
                None => named(name),
            }
        }
        DerivedTerm::Nominal {
            group,
            operands,
            output,
        } => {
            // A family's representation holds its parameters' quantifiers, which no binding writes.
            let names = |operand: &Parametric| {
                !free_variables(types, scratch, (*operand).into()).is_empty()
            };
            if operands.iter().any(names) {
                return named(name);
            }
            let sealed = sealed_nominals(types, scratch, group, operands);
            Ok(sealed.and_then(|sealed| sealed.get(usize::from(output)).map(|ty| (*ty).into())))
        }
        DerivedTerm::Signature { signed, operands } => {
            if operands
                .iter()
                .any(|operand| !free_variables(types, scratch, *operand).is_empty())
            {
                return named(name);
            }
            Ok(Some(
                sealed_signature(types, scratch, signed, operands).into(),
            ))
        }
        DerivedTerm::Carried { .. } | DerivedTerm::Rigid { .. } | DerivedTerm::Open { .. } => {
            named(name)
        }
    }
}

/// The **symbols door** a store names derived names through: each is named by its term's
/// spelling, recorded in the interner the program's refusals render through.
#[derive(Clone, Copy)]
pub struct Spellings<'x> {
    symbols: &'x SymbolInterner,
}

impl<'x> Spellings<'x> {
    pub fn new(symbols: &'x SymbolInterner) -> Self {
        Spellings { symbols }
    }

    /// The derived name of `term`: named by its id and its spelling, between the term's ends.
    pub fn named<'a>(
        &self,
        (types, scratch): (&Chain<'_, '_, '_, '_>, BumpAllocator<'_>),
        term: DerivedTerm<'a>,
    ) -> DerivedName<'a> {
        let spelling = Spelling {
            term,
            types,
            symbols: self.symbols,
        };
        let name = TypeSymbol::spelled(&spelling.to_string(), self.symbols)
            .expect("a term spells parenthesized, with a space");
        let (lower, bound) = term.ends(types, scratch);
        let id = term.id();
        DerivedName {
            id,
            term,
            ty: types.derived(scratch, id, name, lower, bound),
        }
    }
}

/// A term's spelling, as its rule writes it.
struct Spelling<'w, 'a, 'c, 'm, 'n, 'k> {
    term: DerivedTerm<'a>,
    types: &'w Chain<'c, 'm, 'n, 'k>,
    symbols: &'w SymbolInterner,
}

impl std::fmt::Display for Spelling<'_, '_, '_, '_, '_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.term.spell(f, (self.types, self.symbols))
    }
}

/// One derived name: its term's id, its term, and the lattice node that names it, which holds the
/// id, its rendering as its name and its closed ends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DerivedName<'a> {
    pub id: DerivedId,
    pub term: DerivedTerm<'a>,
    pub ty: Parametric,
}

impl DerivedName<'_> {
    /// The name laid through `lay`.
    pub fn laid<'to>(&self, lay: impl Lays<'to>) -> DerivedName<'to> {
        DerivedName {
            id: self.id,
            term: self.term.laid(lay),
            ty: self.ty,
        }
    }
}

/// The derived names one walk laid, sorted by id: the table a reader finds a name's term in.
#[derive(Clone, Copy, Default, Debug)]
pub struct Derived<'a> {
    names: &'a [DerivedName<'a>],
}

impl<'a> Derived<'a> {
    /// The table of a walk that laid no name.
    pub const EMPTY: Derived<'static> = Derived { names: &[] };

    /// `names`, a walk's laid names in the order met, as a table: sorted by id, each once, laid
    /// through `lay`.
    pub fn sorted(
        lay: impl Lays<'a>,
        scratch: BumpAllocator<'_>,
        names: &[DerivedName<'_>],
    ) -> Self {
        let mut sorted = BumpVec::with_capacity_in(names.len(), scratch);
        sorted.extend(names.iter().copied());
        sorted.sort_unstable_by_key(|name| name.id);
        sorted.dedup_by_key(|name| name.id);
        Derived {
            names: collect(lay, sorted.iter().map(|name| name.laid(lay))),
        }
    }

    /// The name whose id is `id`.
    pub fn get(&self, id: DerivedId) -> Option<&'a DerivedName<'a>> {
        let at = (self.names)
            .binary_search_by_key(&id, |name| name.id)
            .ok()?;
        Some(&self.names[at])
    }

    /// Every name, sorted by id.
    pub fn names(&self) -> &'a [DerivedName<'a>] {
        self.names
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The table laid through `lay`, each name's term laid with it.
    pub fn laid<'to>(&self, lay: impl Lays<'to>) -> Derived<'to> {
        Derived {
            names: collect(lay, self.names.iter().map(|name| name.laid(lay))),
        }
    }

    /// Hand `each` every type the table's names and their terms read.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        for name in self.names.iter() {
            each(name.ty.into());
            name.term.each_type(each);
        }
    }
}

#[cfg(test)]
mod tests;
