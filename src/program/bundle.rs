//! Koan's step bundle: what a cell is born holding, what it parks with, and what it keeps in its
//! scratch habitat across a park.
//!
//! A **birth** crosses — a spawn or a tail hands it from one cell to another at the verdict's price
//! — so its family is covariant. A tail's call is laid as a skeleton beside what it hands through
//! kept parts ([`Passing`]): its successor builds its own call over each value, crossed on its own,
//! and keeps a carrier of each ([`Keeps`]). The **parked state** never crosses: a park stores it in the cell's
//! own continuation and the wake hands it back at the same cell's brand, which is why the body
//! runner's state may hold its activation, which binds and so is invariant. See
//! [README.md § The bundle](README.md#the-bundle).

use std::cell::Cell;

use crate::knot::{KActivationView, KValue, Knotted, KnottedFamily};
use crate::memory::{
    CrossedOperand, DropFree, Header, Writer, collect, covariant, reattachable, resident,
};
use crate::scheduler::{Born, Kept, KeptPart, MemoHome, StepBundle};
use crate::scope::RegionHeader;
use crate::scope::{ComposedRoot, Level, Segment, Site, Slot, Stamp};
use crate::symbols::{BinderSymbol, Symbol};
use crate::type_lattice::{DeclaredType, Halves, KType};
use crate::values::{
    ArenaRef, ContentDigest, Half, Record, TypeTiers, Value, Weight, copy_severed,
};

use super::body::Runner;
use super::record::{CallKind, Contract, Evaluated, Program};

/// The bundle a koan program's steps run over.
pub struct KBundle;

/// What a cell is born holding.
#[derive(Clone, Copy)]
pub enum KBirth<'graph, 'cell> {
    /// The top level's root work.
    Program { program: &'graph Program<'graph> },
    /// A call: the frame's first step lays its activation down and binds the parameters from
    /// `arguments`, a record of them by name, checked as `kind` says. A call by name solves each
    /// parameter from its entry in `contributed`, one per parameter in symbol order where the load
    /// recorded any, and from its argument's carried type where that entry is `None` or there are
    /// none. `handed` are the digest memos the caller hands on with arguments it passed bare.
    /// `owed` is the contract of the evaluation that tailed into this frame: `None` when the frame
    /// was spawned. `inherit` is where the asker's decision memo rests, which seeds the frame's
    /// where its home holds none. `level` is the request of the settlement the frame seeks, which a
    /// callable born under it records in its stamp: kept by a hop under its frame's own
    /// settlement; `None` for any other frame, which lays its own from the request the drain
    /// hands it back.
    /// `passing` is what a tail hop hands its successor through kept parts, laid beside the
    /// skeleton the successor's birth is built from ([`Passing`]): `None` everywhere else, and
    /// once the successor has built its own. `kept` is what that successor keeps of its callee
    /// and arguments ([`Keeps`]): `None` for a spawned frame, which keeps what its birth holds.
    Call {
        program: &'graph Program<'graph>,
        callee: KValue<'graph, 'cell>,
        arguments: KValue<'graph, 'cell>,
        kind: CallKind,
        contributed: &'cell [Option<KType>],
        handed: &'cell [Handed],
        owed: Option<Contract>,
        inherit: Option<MemoHome>,
        level: Option<&'cell Level<'cell, Knotted<'graph, 'cell>>>,
        passing: Option<&'cell Passing<'graph, 'cell>>,
        kept: Option<&'cell Keeps<'graph, 'cell>>,
    },
    /// An `EVAL`: the frame's first step lays the code's activation down over the bindings it
    /// carries and the names `offered`, a record of them by name, supplies — and `contract`, the
    /// `EVAL`'s declared return, which the frame owes. `inherit` as a call's.
    Eval {
        program: &'graph Program<'graph>,
        code: KValue<'graph, 'cell>,
        offered: KValue<'graph, 'cell>,
        contract: Contract,
        inherit: Option<MemoHome>,
    },
    /// A `USING` over code composed at run time: one step, under the code's composition, where its
    /// shape lies, fills the holes of `code` from `source` and finishes with the filled code.
    Using {
        program: &'graph Program<'graph>,
        code: KValue<'graph, 'cell>,
        source: KValue<'graph, 'cell>,
    },
    /// An evaluation: what it evaluates, and the view of the activation it reads names through.
    /// A frame's tail hands its last statement over with the frame's `contract`, which the
    /// evaluation then owes its value. `within` is what its frame runs within.
    Evaluate {
        program: &'graph Program<'graph>,
        node: Evaluated,
        view: KActivationView<'graph, 'cell>,
        contract: Option<Contract>,
        within: Within<'graph, 'cell>,
    },
    /// A block — a synthesized block part's shape, or a `USING … SCOPE` body — run beside the
    /// view of the activation it sits in, its last statement's value its own; a `USING` body's
    /// parameters bound to what `module` surfaces, beside the `USING`'s site in the enclosing body,
    /// where the load recorded its picks. `within` as an evaluation's.
    Block {
        program: &'graph Program<'graph>,
        segment: Segment,
        enclosing: KActivationView<'graph, 'cell>,
        module: Option<(KValue<'graph, 'cell>, Site)>,
        within: Within<'graph, 'cell>,
    },
    /// What the top level leaves at rest when it ends: its activation's view, for a later root
    /// work to read a top-level binding through.
    Inspect {
        program: &'graph Program<'graph>,
        view: KActivationView<'graph, 'cell>,
    },
}

/// What a body runs within, handed to every evaluation and block it asks for: whether it runs in
/// the own body of a routing caching frame, whose caching chain it then interns through; the arena
/// node its frame runs under, which a callable born in it is born under; where its frame's decision memo
/// rests, which a block decides in; the memo a frame it calls seeds its own from; and the root of
/// the composed code its frame runs, which a value born in it keeps — the books hold only its
/// digest; the stamp of the chain of settlements its frame's arena node closes, which a
/// callable born in it keeps beside that node — `None` for a frame under its own settlement whose
/// body births no callable; and what its frame keeps of its arguments, callee and binders, which
/// a name read of the frame's body reads beside the value — `None` outside a called frame, and for
/// one called through a barrier.
#[derive(Clone, Copy)]
pub struct Within<'graph, 'cell> {
    pub routes: bool,
    pub build: ArenaRef,
    pub stamp: Option<Stamp<'cell, Knotted<'graph, 'cell>>>,
    pub memo: Option<MemoHome>,
    pub inherit: Option<MemoHome>,
    pub root: Option<ComposedRoot<'graph>>,
    pub kept: Option<&'cell FrameKept<'graph, 'cell>>,
}

/// What a tail hop hands its successor through kept parts, laid beside the birth: each argument the
/// birth's record holds that is a kept part, or a restamp of one, by name, and the frame's own
/// callee, kept, where the hop is into it; and the record's field names as its type names them,
/// in the record's order. The successor's birth is built in its own region, each of these crossing
/// from its kept part and every other argument, and the callee where none is kept, out of the
/// birth.
#[derive(Clone, Copy)]
pub struct Passing<'graph, 'cell> {
    pub callee: Option<Kept<'graph>>,
    pub arguments: &'cell [(Symbol, KeptPart<'graph, 'cell>)],
    pub binders: &'cell [BinderSymbol],
}

/// What a hop's successor keeps of the callee and the arguments its birth holds, built beside the
/// birth: a carrier at each one's home where it pinned, and in the successor where it was copied,
/// the arguments by name.
#[derive(Clone, Copy)]
pub struct Keeps<'graph, 'cell> {
    pub callee: Kept<'graph>,
    pub arguments: &'cell [(Symbol, Kept<'graph>)],
}

/// What a called frame keeps, in a table laid in its region by its first step: a carrier at rest
/// at the true home of each argument, of its callee, and of each binder of its body whose
/// evaluation delivered a value pinned at a home of its own — one cell per slot of its body's
/// shape, empty where it keeps none. `segment` is the body's, which tells a name read landing in
/// the frame's own activation from one landing in a block's.
pub struct FrameKept<'graph, 'cell> {
    pub segment: Segment,
    pub callee: Option<Kept<'graph>>,
    pub slots: &'cell [Cell<Option<Kept<'graph>>>],
}

impl<'graph> FrameKept<'graph, '_> {
    /// What the frame keeps of the value bound at `slot` of its own body.
    pub fn slot(&self, segment: Segment, slot: Slot) -> Option<Kept<'graph>> {
        (segment == self.segment)
            .then(|| self.slots.get(slot.index()).and_then(Cell::get))
            .flatten()
    }
}

impl<'graph, 'cell> Within<'graph, 'cell> {
    /// The top level's before its runner names its memo: no caching frame, and the root.
    pub const TOP: Self = Within {
        routes: false,
        build: ArenaRef::ROOT,
        stamp: None,
        memo: None,
        inherit: None,
        root: None,
        kept: None,
    };

    /// Where a callable born in the body is born: the arena node, and the stamp of its chain.
    pub fn born(&self) -> (ArenaRef, Option<Stamp<'cell, Knotted<'graph, 'cell>>>) {
        (self.build, self.stamp)
    }

    /// What the evaluation a frame hands its last statement to runs within: its own region, under
    /// the frame, so it routes no more, and its frame's arena node stays.
    pub fn handed_off(self) -> Self {
        Within {
            routes: false,
            ..self
        }
    }
}

impl<'graph, 'cell> KBirth<'graph, 'cell> {
    /// Every value the birth carries itself — not those an activation it views holds — for the
    /// crossing's habitat check.
    #[cfg(debug_assertions)]
    fn values(&self, out: &mut impl FnMut(&KValue<'graph, 'cell>)) {
        match self {
            KBirth::Call {
                callee,
                arguments,
                level,
                ..
            } => {
                out(callee);
                out(arguments);
                (level.iter())
                    .flat_map(|level| level.modules)
                    .for_each(&mut *out);
            }
            KBirth::Eval { code, offered, .. } => {
                out(code);
                out(offered);
            }
            KBirth::Using { code, source, .. } => {
                out(code);
                out(source);
            }
            KBirth::Block {
                module: Some((module, _)),
                ..
            } => out(module),
            _ => {}
        }
    }
}

/// A digest memo a call hands its callee: the digest of the value the argument `name` holds, read
/// off the caller's digested slot it was passed bare from, beside the type that value carried
/// there. The callee keeps it only where its parameter binds a value carrying that type.
#[derive(Clone, Copy, Debug)]
pub struct Handed {
    pub name: Symbol,
    pub digest: ContentDigest,
    pub carried: DeclaredType<KType>,
}

/// What a cell holds at a step and parks with.
#[derive(Clone, Copy)]
pub enum KState<'graph, 'cell> {
    /// A cell's first step: what it was born with.
    Born(KBirth<'graph, 'cell>),
    /// The body runner, between units.
    Runner(Runner<'graph, 'cell>),
    /// An evaluator's resumption: what it was born with, and how far it got — which parts of the
    /// node it has asked for is read again off the node itself.
    Evaluating {
        birth: KBirth<'graph, 'cell>,
        stage: u32,
    },
}

/// The family of [`KBirth`].
pub struct KBirthFamily;
reattachable!(KBirthFamily => KBirth<'graph, 'cell>);
// A birth crosses between cells, so its family carries the covariance witness — which is the check
// that an activation's view is covariant in its brand.
covariant!(KBirthFamily);
impl DropFree for KBirthFamily {}

/// The family of [`KState`].
pub struct KStateFamily;
reattachable!(KStateFamily => KState<'graph, 'cell>);

/// The family of what a parked cell keeps in its scratch habitat: the sites a refused tie named,
/// one per evaluation the runner asked for, in the order asked.
pub struct KScratchFamily;
reattachable!(both KScratchFamily => &'scratch [Site]);

impl<'graph> StepBundle<'graph> for KBundle {
    type Birth = KBirthFamily;
    type State = KStateFamily;
    type Scratch = KScratchFamily;

    /// A view is weighed at no bound, so the verdict never copies one: it is a borrow of another
    /// cell's region. An evaluation, a block or an inspection is born under the cell it views —
    /// a frame's hand-off too — where pinning it costs nothing and pledges nothing.
    fn weight<'cell>(birth: &KBirth<'graph, 'cell>) -> usize
    where
        'graph: 'cell,
    {
        match birth {
            KBirth::Program { .. } => size_of::<usize>(),
            KBirth::Call {
                callee,
                arguments,
                contributed,
                handed,
                level,
                passing,
                kept,
                ..
            } => {
                let level =
                    level.map_or(Weight::ZERO, |level| Stamp { level, below: None }.weight());
                // A skeleton's values cross on their own, so the skeleton is what it holds besides.
                let values = match passing {
                    Some(Passing {
                        callee: Some(_), ..
                    }) => Weight::ZERO,
                    Some(_) => callee.weight(),
                    None => callee.weight().plus(arguments.weight()),
                };
                values.plus(level).bytes()
                    + size_of_val(*contributed)
                    + size_of_val(*handed)
                    + kept.map_or(0, |kept| size_of_val(kept.arguments))
            }
            KBirth::Eval { code, offered, .. } => code.weight().plus(offered.weight()).bytes(),
            KBirth::Using { code, source, .. } => code.weight().plus(source.weight()).bytes(),
            KBirth::Evaluate { .. } | KBirth::Block { .. } | KBirth::Inspect { .. } => usize::MAX,
        }
    }

    /// A pinned birth embeds as it is, and its home's branch reaches the cell's: a spawned child's
    /// branch links it, and a hop's successor or a left birth links it too where it has laid no
    /// branch yet, and merges it where it has — a tenant, whose branch is its host's. A copied one
    /// is rebuilt, each type its values carry adopted from the home's branch.
    fn cross<'cell>(
        writer: Writer<'cell, '_>,
        header: Header<'graph, 'cell, '_, RegionHeader>,
        view: &CrossedOperand<'graph, 'cell, '_, KBirthFamily, RegionHeader>,
        born: Born,
        tiers: TypeTiers<'graph>,
    ) -> KBirth<'graph, 'cell>
    where
        'graph: 'cell,
    {
        match view {
            CrossedOperand::Pinned { view, home, .. } => {
                let types = tiers.chain(Halves::alone(Half::of(writer, header)));
                home.read(|header, _| {
                    let Some(branch) = header.into_value().branch() else {
                        return;
                    };
                    match born {
                        Born::Child => types.link(branch),
                        Born::Hop | Born::Left => types.link_or_merge(branch),
                    }
                });
                #[cfg(debug_assertions)]
                view.values(&mut |value| {
                    crate::values::check_pinned_habitat::<KnottedFamily>(&types, value)
                });
                *view
            }
            CrossedOperand::Copied { view: copied, .. } => match *copied {
                KBirth::Program { program } => KBirth::Program { program },
                // The values of one birth cross through one copy, so they share each knot.
                KBirth::Call {
                    program,
                    callee,
                    arguments,
                    kind,
                    contributed,
                    handed,
                    owed,
                    inherit,
                    level,
                    ..
                } => {
                    let types = tiers.chain(Halves::alone(Half::of(writer, header)));
                    let [callee, arguments] = copy_severed::<_, KnottedFamily, _, 2>(
                        writer,
                        &types,
                        view,
                        [&callee, &arguments],
                    );
                    // What a frame keeps names the home it was copied out of: the copy keeps afresh.
                    KBirth::Call {
                        program,
                        callee,
                        arguments,
                        kind,
                        contributed: collect(writer, contributed.iter().copied()),
                        handed: collect(writer, handed.iter().copied()),
                        owed,
                        level: copied_level(writer, &types, view, level),
                        inherit,
                        passing: None,
                        kept: None,
                    }
                }
                KBirth::Eval {
                    program,
                    code,
                    offered,
                    contract,
                    inherit,
                } => {
                    let types = tiers.chain(Halves::alone(Half::of(writer, header)));
                    let [code, offered] = copy_severed::<_, KnottedFamily, _, 2>(
                        writer,
                        &types,
                        view,
                        [&code, &offered],
                    );
                    KBirth::Eval {
                        program,
                        code,
                        offered,
                        contract,
                        inherit,
                    }
                }
                KBirth::Using {
                    program,
                    code,
                    source,
                } => {
                    let types = tiers.chain(Halves::alone(Half::of(writer, header)));
                    let [code, source] = copy_severed::<_, KnottedFamily, _, 2>(
                        writer,
                        &types,
                        view,
                        [&code, &source],
                    );
                    KBirth::Using {
                        program,
                        code,
                        source,
                    }
                }
                // Only a forced copy could reach here with a view, and a view is born under the
                // cell it views, whose crossing pins.
                KBirth::Evaluate { .. } | KBirth::Block { .. } | KBirth::Inspect { .. } => {
                    unreachable!("a birth holding a view is never copied")
                }
            },
        }
    }

    fn born<'cell>(birth: KBirth<'graph, 'cell>) -> KState<'graph, 'cell>
    where
        'graph: 'cell,
    {
        KState::Born(birth)
    }

    /// A call a tail laid a skeleton beside hands on each field of its argument record, in the
    /// record's order, then its callee.
    fn handings<'cell>(
        birth: &KBirth<'graph, 'cell>,
        out: &mut impl FnMut(usize, Option<KeptPart<'graph, 'cell>>),
    ) -> bool
    where
        'graph: 'cell,
    {
        let KBirth::Call {
            arguments,
            passing: Some(passing),
            ..
        } = birth
        else {
            return false;
        };
        let Some((names, _)) = arguments.stored_fields() else {
            return false;
        };
        for (index, name) in names.iter().enumerate() {
            let kept = (passing.arguments.iter()).find(|(passed, _)| passed == name);
            out(index, kept.map(|(_, part)| *part));
        }
        let callee = passing.callee.map(|kept| KeptPart { kept, path: &[] });
        out(names.len(), callee);
        true
    }

    fn handed<'cell>(birth: KBirth<'graph, 'cell>, index: usize) -> Option<KValue<'graph, 'cell>>
    where
        'graph: 'cell,
    {
        let KBirth::Call {
            callee, arguments, ..
        } = birth
        else {
            return None;
        };
        let (_, cells) = arguments.stored_fields()?;
        match cells.get(index) {
            Some(value) => Some(*value),
            None => (index == cells.len()).then_some(callee),
        }
    }

    /// The successor's call, built here over the skeleton and the handed values ([`successor`]):
    /// a pinned skeleton's home reaches the successor's branch, as a pinned birth's does, and a
    /// type a copied one held a value at is adopted from its home where a restamp needs it.
    fn hop<'cell>(
        writer: Writer<'cell, '_>,
        header: Header<'graph, 'cell, '_, RegionHeader>,
        view: &CrossedOperand<'graph, 'cell, '_, KBirthFamily, RegionHeader>,
        handed: &[KValue<'graph, 'cell>],
        kept: &[Kept<'graph>],
        tiers: TypeTiers<'graph>,
    ) -> KBirth<'graph, 'cell>
    where
        'graph: 'cell,
    {
        let types = tiers.chain(Halves::alone(Half::of(writer, header)));
        match view {
            CrossedOperand::Pinned { view, home, .. } => {
                home.read(|header, _| {
                    (header.into_value().branch()).map(|branch| types.link_or_merge(branch))
                });
                let KBirth::Call { level, .. } = view else {
                    unreachable!("a skeleton is a call's")
                };
                successor(writer, &types, view, *level, (handed, kept), &|_| {})
            }
            CrossedOperand::Copied {
                view: copied, home, ..
            } => {
                let KBirth::Call { level, .. } = *copied else {
                    unreachable!("a skeleton is a call's")
                };
                let level = copied_level(writer, &types, view, level);
                let adopt = |ktype: DeclaredType<KType>| {
                    home.read(|header, _| types.adopt(header.into_value().branch(), ktype));
                };
                successor(writer, &types, copied, level, (handed, kept), &adopt)
            }
        }
    }
}

/// A hop's successor's call, laid through `writer` over `skeleton`, the call its predecessor laid
/// beside what it hands on, and `handed`, each handed value at the successor's brand beside the
/// carrier `kept` keeps of it: its arguments a record over the handed values under the skeleton's
/// names, and its callee the last handed value, each restamped at the type the skeleton held it
/// at; what it keeps of each beside it; the rest of the skeleton copied. The record's type is
/// interned over the handed values' types, which the successor resolves; a restamp's type its
/// chain lacks is handed to `adopt` first.
fn successor<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    types: &crate::values::Chain<'cell, 'cell, '_, '_>,
    skeleton: &KBirth<'graph, '_>,
    level: Option<&'cell Level<'cell, Knotted<'graph, 'cell>>>,
    (handed, kept): (&[KValue<'graph, 'cell>], &[Kept<'graph>]),
    adopt: &dyn Fn(DeclaredType<KType>),
) -> KBirth<'graph, 'cell> {
    let KBirth::Call {
        program,
        callee,
        arguments,
        kind,
        contributed,
        handed: memos,
        owed,
        inherit,
        passing: Some(passing),
        ..
    } = skeleton
    else {
        unreachable!("a skeleton is a call laid beside what it hands on")
    };
    let (_, cells) =
        (arguments.stored_fields()).expect("a skeleton's arguments are a record, field by field");
    let held_at = |value: KValue<'graph, 'cell>, ktype: DeclaredType<KType>| {
        if ktype == value.ktype() {
            return value;
        }
        if !types.resolves(ktype) {
            adopt(ktype);
        }
        value.restamped_at(writer, ktype, types)
    };
    let count = passing.binders.len();
    let scratch = crate::memory::Bump::new();
    let mut fields = crate::memory::BumpVec::with_capacity_in(count, &scratch);
    fields
        .extend((0..count).map(|at| (passing.binders[at], held_at(handed[at], cells[at].ktype()))));
    let record = Record::new(writer, &fields, types, &scratch);
    let keeps = Keeps {
        callee: kept[count],
        arguments: writer.fill(count, |at| (passing.binders[at].symbol(), kept[at])),
    };
    KBirth::Call {
        program,
        callee: held_at(handed[count], callee.ktype()),
        arguments: Value::Record(record),
        kind: *kind,
        contributed: collect(writer, contributed.iter().copied()),
        handed: collect(writer, memos.iter().copied()),
        owed: *owed,
        inherit: *inherit,
        level,
        passing: None,
        kept: Some(resident(writer, keeps)),
    }
}

/// `level`, a copied birth's, laid through `writer`: its modules copied through `view`, the
/// birth's copied operand, as the birth's other values are, and each type it names that `types`
/// lacks adopted from the home it was copied out of.
fn copied_level<'graph, 'cell, 'severed>(
    writer: Writer<'cell, '_>,
    types: &crate::values::Chain<'cell, 'cell, '_, '_>,
    view: &CrossedOperand<'graph, 'cell, 'severed, KBirthFamily, RegionHeader>,
    level: Option<&'severed Level<'severed, Knotted<'graph, 'severed>>>,
) -> Option<&'cell Level<'cell, Knotted<'graph, 'cell>>> {
    level.map(|level| {
        let stamp = Stamp { level, below: None }.copied(writer, &mut |module| {
            let [module] = copy_severed::<_, KnottedFamily, _, 1>(writer, types, view, [module]);
            module
        });
        if let CrossedOperand::Copied { home, .. } = view {
            stamp.each_type(&mut |ktype| {
                if !types.resolves(ktype) {
                    home.read(|header, _| types.adopt(header.into_value().branch(), ktype));
                }
            });
        }
        stamp.level
    })
}
