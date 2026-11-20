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
//! its term with the binding written in ([`rebound`]), so a unit record absorbed under a binding and
//! one re-run under it hold one handle. A walk lays each name it meets that no layer beneath it
//! holds ([`Derived`]), and a reader finds a name's term by its id ([`Binds::term`]).
//!
//! A derived name renders as its term: it is named through [`Spellings`], the symbols door, by the
//! spelling its rule gives (`(ELEMENT OF Elt)`). The lattice's `Term` is a type's minting term, a
//! different thing.

use crate::memory::{BumpAllocator, BumpVec, Lays, collect};
use crate::symbols::{SymbolInterner, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, DerivedId, KType, Parametric, PathClass, TypeHandle, TypeNode, bound_above,
    display_name, join,
};
use crate::values::Chain;
use crate::values::digest::{DigestHasher, Tag};

use super::facts::Unknown;
use super::nominal::{Handed, Nominals, sealed_nominals};
use super::question::{Answer, Question, answer, free_variables, rebound};
use super::record_types::Closed;
use super::rule::Rule;
use super::signed::{Signed, sealed_signature};
use super::typed::Resolved;

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
    /// The type the module a binding binds to the `:Module` binder `binder`, named `name`, carries:
    /// a signature at every binding, which no other node spells.
    Carried { binder: Unknown, name: ValueSymbol },
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
            DerivedTerm::Carried { binder, .. } => {
                hasher.count(4).feed(binder);
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
            DerivedTerm::Carried { binder, name } => DerivedTerm::Carried { binder, name },
        };
        (term, changed)
    }

    /// Closed ends holding what every binding gives the term: a rule's, or for a decided name
    /// `Never` up to the join of its arms' bounds.
    pub fn ends(
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
            DerivedTerm::Signature { .. } | DerivedTerm::Carried { .. } => {
                (KType::NEVER, KType::EMPTY_SIGNATURE)
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
            DerivedTerm::Carried { name, .. } => {
                write!(f, "(CARRIED BY {})", symbols.display(name.symbol()))
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
            DerivedTerm::Carried { binder, name } => DerivedTerm::Carried { binder, name },
        }
    }
}

/// **The type `term` hands back**: a rule's closed output where every operand is closed, else what
/// the rule writes over the names, else `name`'s name for it; a decided term's one arm where every
/// arm is it, the arm of its rigid answer ([`rigid_answer`]), else `name`'s. A door hands back what
/// this gives its term, and a binding gives a derived name what this gives its term with the
/// binding written in, so a rule hands back one handle at load and under every binding. `None`
/// where the closed rule has no answer over closed operands, or `name` names none.
pub fn handed(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    term: DerivedTerm<'_>,
    name: &mut dyn FnMut(DerivedTerm<'_>) -> Option<Parametric>,
) -> Option<Parametric> {
    match term {
        DerivedTerm::Output {
            rule,
            operands,
            output,
        } => {
            let mut closed = BumpVec::with_capacity_in(operands.len(), scratch);
            closed.extend(operands.iter().map_while(|ty| types.concrete_declared(*ty)));
            if closed.len() == operands.len() {
                let outputs = rule.closed(types, scratch, &closed)?;
                return outputs.get(usize::from(output)).map(|ty| (*ty).into());
            }
            let spelled = rule.spelled(types, scratch, operands);
            match spelled.get(usize::from(output)).copied().flatten() {
                Some(written) => Some(written),
                None => name(term),
            }
        }
        DerivedTerm::Decided {
            decision: Decider::Answer(question),
            arms,
        } => {
            if let [first, rest @ ..] = arms
                && rest.iter().all(|arm| arm == first)
            {
                return Some(*first);
            }
            match rigid_answer(types, scratch, &question) {
                Some(answer) => arms.get(answer.index_in(&question) as usize).copied(),
                None => name(term),
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
                return name(term);
            }
            let sealed = sealed_nominals(types, scratch, group, operands)?;
            sealed.get(usize::from(output)).map(|ty| (*ty).into())
        }
        DerivedTerm::Signature { signed, operands } => {
            if operands
                .iter()
                .any(|operand| !free_variables(types, scratch, *operand).is_empty())
            {
                return name(term);
            }
            Some(sealed_signature(types, scratch, signed, operands).into())
        }
        DerivedTerm::Carried { .. } => name(term),
    }
}

/// The answer every binding gives `question`, where one does: the closed door's over closed types,
/// *yes* for two types that are one, *no* for a type that is `Never` at no binding against
/// `Never`, and the kind of a type whose head fixes it.
pub fn rigid_answer(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    question: &Question<'_>,
) -> Option<Answer> {
    let mut closed = true;
    question.each_type(&mut |ty| closed &= types.concrete_declared(ty).is_some());
    if closed {
        return answer(types, scratch, question);
    }
    let never = DeclaredType::Type(Parametric::from(KType::NEVER));
    match *question {
        Question::Same { a, b } if a == b => Some(Answer::Bool(true)),
        Question::Same { a, b } if b == never && !may_be_never(types, a) => {
            Some(Answer::Bool(false))
        }
        Question::Same { a, b } if a == never && !may_be_never(types, b) => {
            Some(Answer::Bool(false))
        }
        Question::Kind { ty } if !kind_varies(types, ty) => {
            Some(Answer::Kind(bound_above(types, scratch, ty).kind_of(types)))
        }
        Question::Runs { .. } | Question::Arms { .. } => answer(types, scratch, question),
        Question::Overlaps {
            registered,
            against,
        } if super::question::never_overlaps(types, scratch, registered, against) => {
            Some(Answer::Overlaps(None))
        }
        // A signature is one at every pin, so a field read over it reads a module member.
        Question::Defined {
            rule: super::Rule::Fielded,
            operands: [DeclaredType::Type(ty)],
        } if matches!(
            types.node(*ty),
            TypeNode::Signature { .. }
                | TypeNode::SignatureApply { .. }
                | TypeNode::SignatureMeet { .. }
        ) =>
        {
            Some(Answer::Bool(true))
        }
        // A run-bound name's binding lies under its bound, which a carrier never does: where the
        // bound declares a value member, so does every binding's module, `Never` aside.
        Question::Defined {
            rule: super::Rule::SignatureValue(name),
            operands: [DeclaredType::Type(ty)],
        } => match types.node(*ty) {
            TypeNode::Lexical { bound, .. } => (bound != KType::NEVER
                && super::heads::signature_value(types, scratch, bound, name).is_some())
            .then_some(Answer::Bool(true)),
            _ => None,
        },
        // A head no binding changes says whether a type is a function.
        Question::IsFunction { ty } => match types.node(ty) {
            TypeNode::KFunction { .. } => Some(Answer::Bool(true)),
            TypeNode::Union { .. } => None,
            node if is_variable(&node) => None,
            _ => Some(Answer::Bool(false)),
        },
        _ => None,
    }
}

/// Whether some binding makes `ty` `Never`: a variable may be bound to it, and a union is it where
/// every member is; any other head is never `Never`.
pub fn may_be_never(types: &Chain<'_, '_, '_, '_>, ty: DeclaredType<Parametric>) -> bool {
    let DeclaredType::Type(ty) = ty else {
        return false;
    };
    if let Some(closed) = types.concrete(ty) {
        return closed == KType::NEVER;
    }
    match types.node(ty) {
        TypeNode::Union { members } => members
            .iter()
            .all(|member| may_be_never(types, DeclaredType::Type(member))),
        node => is_variable(&node),
    }
}

/// Whether a binding decides the kind of `ty`: a variable, or a union holding one.
pub fn kind_varies(types: &Chain<'_, '_, '_, '_>, ty: Parametric) -> bool {
    bare_variable(types, DeclaredType::Type(ty))
        || (matches!(types.node(ty), TypeNode::Union { .. }) && types.concrete(ty).is_none())
}

/// Whether `ty` is a variable itself, not a type built over one.
pub fn bare_variable(types: &Chain<'_, '_, '_, '_>, ty: DeclaredType<Parametric>) -> bool {
    match ty {
        DeclaredType::Type(ty) => types.concrete(ty).is_none() && is_variable(&types.node(ty)),
        DeclaredType::Scheme(_) => false,
    }
}

/// Whether `node` is a variable's own node.
pub fn is_variable<H: TypeHandle>(node: &TypeNode<'_, H>) -> bool {
    matches!(
        node,
        TypeNode::Quantified { .. }
            | TypeNode::Lexical { .. }
            | TypeNode::Path { .. }
            | TypeNode::Derived { .. }
            | TypeNode::Parameter { .. }
            | TypeNode::Projection { .. }
    )
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

/// The derived names one walk laid that no layer beneath it holds, sorted by id: the table a
/// reader finds a name's term in.
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
