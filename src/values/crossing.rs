//! Moving a value between regions: one verb over the substrate's two placement doors, and the
//! copy-or-pin verdict a graph is built with.
//!
//! A pinned operand arrives at the destination's region lifetime and is embedded as it is. A copied
//! one arrives at an unrelated lifetime, so the only rebuild that typechecks is a deep one through
//! the destination's writer, and a knot member rebuilds its whole knot through its family; what a
//! member borrows of program storage embeds verbatim.
//!
//! The deep copy ([`Copying`]) reads the value through [the door](super::surface): it lays down only
//! what each part's seen type names, at that type, and weighs what it lays down, so a copy drops
//! what a retype hid. A knot's data node seen at its own memo copies with its knot; one seen at
//! another type is laid down as a plain value of its kind, as a retype lays it down.
//!
//! It runs over an explicit stack in a bump of its own, so the stack it
//! uses does not grow with the value's depth: a composite is a frame whose children are copied
//! before it is laid down, and a knot member is a frame over the values its family
//! [lists](KnottedFamily::held), tied once they are all copied. One copy serves one placement and
//! keys every knot it rebuilds by its root, so two references to one knot arrive as members of one
//! copy of it. Plain values reached twice still copy twice.
//!
//! An adoption ([`adopt_value`]) is the other walk: a value leaving a caching body keeps its words,
//! and only the type memos it reaches are made to resolve in the region it leaves for. It stops at a part whose memo bits ([settled.rs](settled.rs)) say its memos
//! already resolve there, and an exit marks what it walked settled after. It reads a value's bits
//! before laying anything, and lays its walk in its caller's step scratch.

use crate::memory::{
    Active, Bump, BumpAllocator, BumpBackedMap, BumpBackedSet, BumpVec, CellHandle, Covariant,
    CrossedOperand, Delivery, HomeHeader, Operand, Prices, Projects, Reattachable,
    ReattachableOverBoth, Stale, StepContext, Verdict, Writer, bump_set, bump_table, collect,
};
use crate::type_lattice::{Branch, DeclaredType, Halves, KType, Parametric};

use super::habitat::{BranchCell, Chain, Half, TypeTiers, Types};
use super::settled::stamp_knot;
use super::surface::Parts;
use super::{
    Dict, Knotted, KnottedFamily, List, MemoBits, Record, Retyped, Seen, Surface, Tagged, Value,
    ValueCarrier, ValueFamily, text,
};

/// How many copy bytes one pin byte is worth: an operand copies while its copy costs less than a
/// `COPY_RATIO`th of what pinning it would newly retain.
pub const COPY_RATIO: usize = 4;

/// The crossing verdict: copy when `copy_bytes * COPY_RATIO < pin_bytes`, saturating, pin otherwise.
/// The occupancy the prices also carry does not move it.
pub fn verdict(prices: Prices) -> Verdict {
    if prices.copy_bytes.saturating_mul(COPY_RATIO) < prices.pin_bytes {
        Verdict::Copy
    } else {
        Verdict::Pin
    }
}

/// Build `carrier`'s value into `dest`'s region and hand back the carrier resting there: the value
/// as it is under a pin, rebuilt deep under a copy, its types made to resolve there over `tiers`.
pub fn cross<
    'graph,
    'step,
    C: Reattachable<'graph>,
    S: ReattachableOverBoth<'graph>,
    D: Delivery<'graph>,
    XF: KnottedFamily<'graph>,
    H: Projects<'graph, Projection = BranchCell>,
>(
    context: &mut StepContext<'graph, 'step, '_, '_, '_, C, S, D, H>,
    dest: impl Into<CellHandle>,
    carrier: &ValueCarrier<'graph, 'step, XF>,
    tiers: TypeTiers<'graph>,
) -> Result<ValueCarrier<'graph, 'step, XF>, Stale<CellHandle>>
where
    ValueFamily<XF>: Covariant<'graph>,
{
    let weight = context.read(carrier).value().weight();
    context.alloc_into::<ValueFamily<XF>, ValueFamily<XF>>(
        dest,
        &[Operand {
            carrier,
            copy_bytes: weight.bytes(),
        }],
        |writer, header, views| {
            let types = tiers.chain(Halves::alone(Half::of(writer, header)));
            Active::new(cross_view(writer, &types, &views[0]))
        },
    )
}

/// The own-cell crossing: `carrier`'s value made reachable at the executing cell's `'here`, where it
/// may be embedded in what the step builds or captured by its continuation, its types made to
/// resolve there over `tiers`.
pub fn cross_here<
    'graph,
    'step,
    'here,
    C: Reattachable<'graph>,
    S: ReattachableOverBoth<'graph>,
    D: Delivery<'graph>,
    XF: KnottedFamily<'graph>,
    H: Projects<'graph, Projection = BranchCell>,
>(
    context: &mut StepContext<'graph, 'step, 'here, '_, '_, C, S, D, H>,
    carrier: &ValueCarrier<'graph, 'step, XF>,
    tiers: TypeTiers<'graph>,
    check: Option<&Chain<'_, '_, '_, '_>>,
) -> Value<'here, XF::Closed<'here>>
where
    ValueFamily<XF>: Covariant<'graph>,
{
    let weight = context.read(carrier).value().weight();
    context.alloc_here(
        &[Operand {
            carrier,
            copy_bytes: weight.bytes(),
        }],
        |writer, header, views| {
            let types = tiers.chain(Halves::alone(Half::of(writer, header)));
            cross_view_against(writer, &types, &views[0], check)
        },
    )
}

/// One crossed operand as a value at the destination's brand, every type it carries made to
/// resolve in `types`, the destination's chain: a pinned view as it is, its home's branch merged
/// into the destination's; a copied view rebuilt through `writer`, each part's memo adopted from
/// its home's branch before it is read. The one door a copy of a value comes through, and it takes
/// a [`CrossedOperand`], which only a priced placement hands out — so every copy is one the graph
/// priced. See [README.md § Crossing](README.md#crossing).
pub fn cross_view<
    'graph,
    'cell,
    XF: KnottedFamily<'graph>,
    H: Projects<'graph, Projection = BranchCell>,
>(
    writer: Writer<'cell, '_>,
    types: &Chain<'cell, 'cell, '_, '_>,
    view: &CrossedOperand<'graph, 'cell, '_, ValueFamily<XF>, H>,
) -> Value<'cell, XF::Closed<'cell>> {
    cross_view_against(writer, types, view, None)
}

/// [`cross_view`], its debug check of a pinned value reading `check` where one is given rather
/// than `types`: a value crossing within a caching body carries types in the body's scratch
/// tiers, which the body's own chain reads and the destination's region chain does not.
pub fn cross_view_against<
    'graph,
    'cell,
    XF: KnottedFamily<'graph>,
    H: Projects<'graph, Projection = BranchCell>,
>(
    writer: Writer<'cell, '_>,
    types: &Chain<'cell, 'cell, '_, '_>,
    view: &CrossedOperand<'graph, 'cell, '_, ValueFamily<XF>, H>,
    check: Option<&Chain<'_, '_, '_, '_>>,
) -> Value<'cell, XF::Closed<'cell>> {
    match *view {
        CrossedOperand::Pinned {
            view: value, home, ..
        } => {
            home.read(|header, _| {
                if let Some(branch) = H::project(header.into_value()).get() {
                    types.merge(branch);
                }
            });
            #[cfg(debug_assertions)]
            match check {
                Some(check) => check_pinned_habitat::<XF>(check, &value),
                None => check_pinned_habitat::<XF>(types, &value),
            }
            #[cfg(not(debug_assertions))]
            let _ = check;
            value
        }
        CrossedOperand::Copied {
            view: value, home, ..
        } => {
            let [copied] = copied::<XF, H, 1>(writer, types, home, [&value]);
            #[cfg(debug_assertions)]
            check_habitat::<XF>(types, &copied);
            copied
        }
    }
}

/// Values held inside a copied operand of another family — a birth that carries values — rebuilt
/// through `writer`, all through one copy, so they share one copy of each knot they reach, every
/// type they carry adopted into `types` from the operand's home branch. The operand is the proof:
/// only a priced placement hands one out, and its severed brand is the one `values` are read at,
/// so this is still a copy the graph priced.
pub fn copy_severed<
    'graph,
    'cell,
    'severed,
    F: Reattachable<'graph>,
    XF: KnottedFamily<'graph>,
    H: Projects<'graph, Projection = BranchCell>,
    const N: usize,
>(
    writer: Writer<'cell, '_>,
    types: &Chain<'cell, 'cell, '_, '_>,
    operand: &CrossedOperand<'graph, 'cell, 'severed, F, H>,
    values: [&Value<'severed, XF::Closed<'severed>>; N],
) -> [Value<'cell, XF::Closed<'cell>>; N] {
    let CrossedOperand::Copied { home, .. } = *operand else {
        unreachable!("a pinned operand's values embed as they are")
    };
    let copies = copied::<XF, H, N>(writer, types, home, values);
    #[cfg(debug_assertions)]
    for copy in &copies {
        check_habitat::<XF>(types, copy);
    }
    copies
}

/// `values` rebuilt through `writer`, all through one copy, each type a part carries adopted into
/// `types` from `home`'s branch, the half they were read in.
fn copied<
    'graph,
    'cell,
    'from,
    XF: KnottedFamily<'graph>,
    H: Projects<'graph, Projection = BranchCell>,
    const N: usize,
>(
    writer: Writer<'cell, '_>,
    types: &Chain<'cell, 'cell, '_, '_>,
    home: HomeHeader<'graph, 'from, H>,
    values: [&Value<'from, XF::Closed<'from>>; N],
) -> [Value<'cell, XF::Closed<'cell>>; N]
where
    'graph: 'from,
{
    home.read(|header, _| {
        rebuilt::<XF, N>(writer, types, H::project(header.into_value()).get(), values)
    })
    .unwrap_or_else(|| rebuilt::<XF, N>(writer, types, None, values))
}

/// [`copied`], its types adopted from `source`, the home's branch, or from nowhere where the home
/// laid none.
fn rebuilt<'graph, 'cell, 'from, XF: KnottedFamily<'graph>, const N: usize>(
    writer: Writer<'cell, '_>,
    types: &Chain<'cell, 'cell, '_, '_>,
    source: Option<&Branch<'_>>,
    values: [&Value<'from, XF::Closed<'from>>; N],
) -> [Value<'cell, XF::Closed<'cell>>; N]
where
    'graph: 'from,
{
    let scratch = Bump::new();
    let mut copying = Copying::<XF>::new(writer, types, source, &scratch);
    #[cfg(test)]
    let before = writer.in_use();
    let copies = values.map(|value| copying.value(value));
    #[cfg(test)]
    COPIED.with(|copied| copied.set(copied.get() + writer.in_use() - before));
    copies
}

/// How many parts of a pinned value the habitat check reads: a pin is O(1) in a release build, so
/// a debug build reads a bounded prefix of the value rather than make every pin of a deep value
/// walk it whole, which turns a loop over a deep value quadratic. A copy already walks every part
/// it lays, so its check reads them all.
#[cfg(debug_assertions)]
const PINNED_PARTS: usize = 16;

/// The habitat check, in a debug build: every type `value` carries — each part's own memo, hidden
/// parts included, each type value's payload, and every type a knot it reaches carries — resolves
/// in `types`, the chain of the habitat it was crossed into. It interns nothing and caches nothing,
/// so a debug build's branches hold what a release build's would. For a copy, which walked the
/// value already.
#[cfg(debug_assertions)]
pub fn check_habitat<'graph, 'cell, XF: KnottedFamily<'graph>>(
    types: &Chain<'_, '_, '_, '_>,
    value: &Value<'cell, XF::Closed<'cell>>,
) {
    check_parts::<XF>(types, value, usize::MAX, Cutoff::Off);
}

/// [`check_habitat`] for a pinned value, over the first [`PINNED_PARTS`] parts a depth-first walk
/// reaches: a crossing door that misses a tier misses every type the tier holds, so a prefix finds
/// it unless every such type lies past the prefix.
#[cfg(debug_assertions)]
pub fn check_pinned_habitat<'graph, 'cell, XF: KnottedFamily<'graph>>(
    types: &Chain<'_, '_, '_, '_>,
    value: &Value<'cell, XF::Closed<'cell>>,
) {
    check_parts::<XF>(types, value, PINNED_PARTS, Cutoff::Off);
}

/// The habitat check over the first `limit` parts of `value`, stopping where `cutoff` says. It lays
/// its walk in a bump of its own: it is not the walk whose cost the caller pays.
#[cfg(debug_assertions)]
fn check_parts<'graph, 'cell, XF: KnottedFamily<'graph>>(
    types: &Chain<'_, '_, '_, '_>,
    value: &Value<'cell, XF::Closed<'cell>>,
    limit: usize,
    cutoff: Cutoff,
) {
    carried::<XF>(
        value,
        limit,
        cutoff,
        &Bump::new(),
        &mut |kt| {
            assert!(
                types.resolves(kt),
                "a crossed value carries a type its habitat cannot resolve: {kt:?} in {}",
                match value {
                    Value::Knotted(_) => "knotted",
                    Value::Record(_) => "record",
                    Value::List(_) => "list",
                    Value::Type(_) => "type",
                    Value::Retyped(_) => "retyped",
                    _ => "other",
                }
            );
        },
        &mut |_| {},
        &mut |_| {},
    );
}

/// Where [`carried`] stops at a part whose [bits](super::MemoBits) meet the contract.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cutoff {
    /// Nowhere: every part is entered and every memo handed over.
    Off,
    /// At the part: it hands over nothing and is not entered.
    At,
}

/// Every type `value` carries over its first `limit` parts, handed to `each`: each part's own memo,
/// hidden parts included, each type value's payload, and every type a knot it reaches carries — so
/// every closure's and quote's captures and every knot member. A part whose
/// [bits](super::MemoBits) meet the contract is handed to `skipped` and not entered, handing over
/// nothing under [`Cutoff::At`]; a header's bits are its member's, so it always hands over its own memo. Every composite and knot
/// member the walk does enter is handed to `entered`, a knot once. The top value is read before
/// anything is laid, so one with no part or whose bits meet costs no allocation; the walk's stack
/// and knot set live in `scratch`.
fn carried<'graph, 'cell, XF: KnottedFamily<'graph>>(
    value: &Value<'cell, XF::Closed<'cell>>,
    limit: usize,
    cutoff: Cutoff,
    scratch: BumpAllocator<'_>,
    each: &mut dyn FnMut(DeclaredType<Parametric>),
    skipped: &mut dyn FnMut(Value<'cell, XF::Closed<'cell>>),
    entered: &mut dyn FnMut(Value<'cell, XF::Closed<'cell>>),
) {
    if !reached(value, cutoff, each, skipped) {
        return;
    }
    let mut stack = BumpVec::new_in(scratch);
    let mut knots: BumpBackedSet<'_, _> = bump_set(scratch);
    opened::<XF>(*value, &mut knots, &mut stack, each, entered);
    let mut read = 1;
    while let Some(value) = stack.pop()
        && read < limit
    {
        read += 1;
        if reached(&value, cutoff, each, skipped) {
            opened::<XF>(value, &mut knots, &mut stack, each, entered);
        }
    }
}

/// Hand `each` what `value` itself carries, as `cutoff` says, and answer whether [`carried`] enters
/// it: a part with no parts, or whose bits meet, is not entered.
fn reached<'cell, X: Knotted>(
    value: &Value<'cell, X>,
    cutoff: Cutoff,
    each: &mut dyn FnMut(DeclaredType<Parametric>),
    skipped: &mut dyn FnMut(Value<'cell, X>),
) -> bool {
    let meets = cutoff != Cutoff::Off && bits(value).is_some_and(MemoBits::meets);
    // A header's bits are its member's, so its own memo is handed over at every cutoff.
    if !meets || matches!(value, Value::Retyped(_)) {
        each(value.ktype().into());
    }
    if let Value::Type(type_value) = value {
        each(type_value.handle().into());
    }
    if meets {
        skipped(*value);
        return false;
    }
    matches!(
        value,
        Value::List(_)
            | Value::Dict(_)
            | Value::Record(_)
            | Value::Tagged(_)
            | Value::Knotted(_)
            | Value::Retyped(_)
    )
}

/// Enter `value`, a part [`reached`] answered for: a knot, the first time the walk meets it, hands
/// `each` the types it carries and stacks the values it holds; a composite stacks its parts.
fn opened<'graph, 'cell, XF: KnottedFamily<'graph>>(
    value: Value<'cell, XF::Closed<'cell>>,
    knots: &mut BumpBackedSet<'_, XF::Closed<'cell>>,
    stack: &mut BumpVec<'_, Value<'cell, XF::Closed<'cell>>>,
    each: &mut dyn FnMut(DeclaredType<Parametric>),
    entered: &mut dyn FnMut(Value<'cell, XF::Closed<'cell>>),
) {
    match value {
        Value::Knotted(_) | Value::Retyped(_) => {
            let member = value.knot_member().expect("a knot member");
            if knots.insert(member.root()) {
                entered(value);
                XF::carried_types(&member, &mut |kt| each(kt));
                XF::held(&member, &mut |held| stack.push(held));
            }
        }
        Value::List(_) | Value::Dict(_) | Value::Record(_) | Value::Tagged(_) => {
            entered(value);
            super::surface::raw_parts(value, &mut |part| stack.push(part));
        }
        Value::Number(_) | Value::Bool(_) | Value::Null | Value::Str(_) | Value::Type(_) => {
            unreachable!("a value with no part is not entered")
        }
    }
}

/// The bits a part the walk reaches carries: a composite's own, or a knot member's knot's; `None`
/// for a leaf or a type value, which has no part to enter.
fn bits<X: Knotted>(value: &Value<'_, X>) -> Option<MemoBits> {
    match value {
        Value::Number(_) | Value::Bool(_) | Value::Null | Value::Str(_) | Value::Type(_) => None,
        Value::List(list) => Some(list.bits()),
        Value::Dict(dict) => Some(dict.bits()),
        Value::Record(record) => Some(record.bits()),
        Value::Tagged(tagged) => Some(tagged.bits()),
        Value::Knotted(member) => Some(member.bits()),
        Value::Retyped(header) => Some(header.member().bits()),
    }
}

/// Mark `value`, which an exit's walk entered, settled: every memo it reaches now resolves in its
/// home's branch, or below a part that says so itself.
fn settle<X: Knotted>(value: &Value<'_, X>) {
    match value {
        Value::List(list) => list.settle(),
        Value::Dict(dict) => dict.settle(),
        Value::Record(record) => record.settle(),
        Value::Tagged(tagged) => tagged.settle(),
        Value::Knotted(member) => settle_knot(member),
        Value::Retyped(header) => settle_knot(&header.member()),
        Value::Number(_) | Value::Bool(_) | Value::Null | Value::Str(_) | Value::Type(_) => {}
    }
}

fn settle_knot<X: Knotted>(member: &X) {
    member.set_bits(MemoBits {
        settled: true,
        ..member.bits()
    });
}

/// Make every type `value` carries resolve in `region`, the region chain of the step whose caching
/// body the value leaves, adopting it from `source`, the chain the body runs on — the one door at a
/// caching body's exits. The walk does not enter a part whose bits meet the contract: a root-closed
/// part's memos resolve everywhere, and a settled one's in its home's branch, which every chain
/// reading the value reads. Such a part hands over nothing, its own memo included; a header, whose
/// bits are its member's, always its own memo. Every value it entered is marked settled after. The
/// walk lays its stack, knot set and settled list in `scratch`, its caller's step scratch, and lays
/// nothing for a value whose bits meet. A test build audits each part as it is skipped, and a
/// debug build checks what it adopted resolves, with the same cutoff. See [design/type-tiers.md § An adoption stops at a settled part](../../design/type-tiers.md#an-adoption-stops-at-a-settled-part).
pub fn adopt_value<'graph, 'cell, XF: KnottedFamily<'graph>>(
    region: &Types<'_, '_>,
    source: &Chain<'_, '_, '_, '_>,
    value: &Value<'cell, XF::Closed<'cell>>,
    scratch: BumpAllocator<'_>,
) {
    #[cfg(test)]
    EXIT_WALKS.with(|count| count.set(count.get() + 1));
    let mut entered = BumpVec::new_in(scratch);
    carried::<XF>(
        value,
        usize::MAX,
        Cutoff::At,
        scratch,
        &mut |kt| {
            #[cfg(test)]
            ADOPTED.with(|adopted| adopted.set(adopted.get() + 1));
            region.adopt_from(source, kt);
        },
        &mut |part| {
            #[cfg(test)]
            {
                PARTS_SKIPPED.with(|count| count.set(count.get() + 1));
                audit::<XF>(region, &part);
            }
            #[cfg(not(test))]
            let _ = part;
        },
        &mut |part| {
            #[cfg(test)]
            PARTS_ENTERED.with(|count| count.set(count.get() + 1));
            entered.push(part);
        },
    );
    #[cfg(debug_assertions)]
    check_parts::<XF>(region, value, usize::MAX, Cutoff::At);
    entered.iter().for_each(settle);
}

/// The test build's audit of a part an adoption skips, as it skips it: walked whole in a bump of its
/// own, every memo its bits answer for must lie in the root where it is root-closed, and resolve in
/// `region`, the chain of the region the value lies in, where it is settled.
#[cfg(test)]
fn audit<'graph, 'cell, XF: KnottedFamily<'graph>>(
    region: &Chain<'_, '_, '_, '_>,
    part: &Value<'cell, XF::Closed<'cell>>,
) {
    let bits = bits(part).expect("only a part with bits is skipped");
    // A header's bits answer for its member, not for the header's own memo, which was adopted.
    let part = &match part {
        Value::Retyped(header) => Value::Knotted(header.member()),
        other => *other,
    };
    carried::<XF>(
        part,
        usize::MAX,
        Cutoff::Off,
        &Bump::new(),
        &mut |kt| {
            if bits.root_closed {
                assert!(
                    region.in_root(kt),
                    "a root-closed part {:?} reaches {kt:?}, which is no root type",
                    part.ktype(),
                );
            } else {
                assert!(
                    region.resolves(kt),
                    "a settled part reaches {kt:?}, which its home region cannot resolve"
                );
            }
        },
        &mut |_| {},
        &mut |_| {},
    );
}

#[cfg(test)]
thread_local! {
    /// How many handles [`adopt_value`] has adopted: what a test reads to see a settled value's
    /// parts left unwalked.
    pub static ADOPTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many walks [`adopt_value`] has run into a region, at an exit.
    pub static EXIT_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many parts an adoption's walk has entered: each composite, and each knot once a walk.
    pub static PARTS_ENTERED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many parts an adoption's walk has skipped on their bits.
    pub static PARTS_SKIPPED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many bytes every deep copy has laid in its destination: what a test reads to see what
    /// a loop copies per hop.
    pub static COPIED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// One pending composite of the deep copy: the surface its children are read from, how many of
/// them have been started, and where its finished children begin on the copy's `done` stack.
#[derive(Clone, Copy)]
enum Frame<'x, 'from, X> {
    /// A list, dict, record or tagged value, opened at the type it is seen at.
    Composite {
        surface: Surface<'x, 'from, X>,
        next: usize,
        base: usize,
    },
    /// A knot member whose knot is not yet copied: its held values sit on the copy's `held` stack
    /// from `held` to `end`, and `retype` is the type it is seen at where that is not its own.
    Knot {
        member: X,
        retype: Option<DeclaredType<KType>>,
        held: usize,
        end: usize,
        next: usize,
        base: usize,
    },
}

/// The deep copy of one placement: every region part of a value rebuilt through `writer` at the
/// type it is seen at, laying down only what that type names and weighing what it lays down; every
/// program node embedded as the same node; a knot member rebuilt by its family once per knot, its
/// memos and weights carried over and its memo bits stamped where it lands. A composite's bits are
/// computed by the door that lays it, through the destination's chain. Total. Built only by [`cross_view`] and
/// [`copy_severed`], so every copy is one the graph priced.
struct Copying<'graph, 'x, 'from, 'to, 'home, 'reg, XF: KnottedFamily<'graph>>
where
    'graph: 'from,
    'graph: 'to,
{
    writer: Writer<'to, 'x>,
    /// The destination's chain: every type the copy reads is adopted into it, then read there.
    types: &'x Chain<'to, 'to, 'reg, 'x>,
    /// The home branch the copied value's types are read from; `None` where its home had none.
    source: Option<&'x Branch<'home>>,
    /// Where the surfaces a copy opens are staged, beside its stacks.
    scratch: BumpAllocator<'x>,
    frames: BumpVec<'x, Frame<'x, 'from, XF::Closed<'from>>>,
    /// Finished copies whose composite is still pending.
    done: BumpVec<'x, Value<'to, XF::Closed<'to>>>,
    /// The values each pending knot holds, as its family listed them.
    held: BumpVec<'x, Value<'from, XF::Closed<'from>>>,
    /// Each knot copied so far: its root, beside its copy's root.
    knots: BumpBackedMap<'x, XF::Closed<'from>, XF::Closed<'to>>,
}

impl<'graph, 'x, 'from, 'to, 'home, 'reg, XF: KnottedFamily<'graph>>
    Copying<'graph, 'x, 'from, 'to, 'home, 'reg, XF>
where
    'graph: 'from,
    'graph: 'to,
{
    fn new(
        writer: Writer<'to, 'x>,
        types: &'x Chain<'to, 'to, 'reg, '_>,
        source: Option<&'x Branch<'home>>,
        scratch: BumpAllocator<'x>,
    ) -> Self {
        Copying {
            writer,
            types,
            source,
            scratch,
            frames: BumpVec::new_in(scratch),
            done: BumpVec::new_in(scratch),
            held: BumpVec::new_in(scratch),
            knots: bump_table(scratch),
        }
    }

    /// Make `kt` resolve in the destination, re-laying from the source what it lacks.
    fn adopt(&self, kt: impl Into<DeclaredType<Parametric>>) {
        self.types.adopt(self.source, kt);
    }

    /// `value` copied: a leaf at once, anything else by running the stack until it empties. Its
    /// own memo is adopted before anything reads it.
    fn value(&mut self, value: &Value<'from, XF::Closed<'from>>) -> Value<'to, XF::Closed<'to>> {
        self.adopt(value.ktype());
        if let Some(leaf) = self.start(Seen::of(*value)) {
            return leaf;
        }
        while let Some(&frame) = self.frames.last() {
            match self.next_child(frame) {
                Some(child) => {
                    self.advance();
                    if let Some(leaf) = self.start(child) {
                        self.done.push(leaf);
                    }
                }
                None => {
                    self.frames.pop();
                    let finished = self.finish(frame);
                    if self.frames.is_empty() {
                        return finished;
                    }
                    self.done.push(finished);
                }
            }
        }
        unreachable!("a started composite pushes a frame")
    }

    /// Begin copying `seen`: a leaf, or a knot already copied, is finished at once; any other
    /// value pushes its frame. A knot member copies with its knot, bare or under a header, and one
    /// seen at a type other than its own arrives as a header at that type.
    fn start(
        &mut self,
        seen: Seen<'from, XF::Closed<'from>>,
    ) -> Option<Value<'to, XF::Closed<'to>>> {
        let writer = self.writer;
        let base = self.done.len();
        let value = seen.value();
        let finished = match value {
            Value::List(_) | Value::Dict(_) | Value::Record(_) | Value::Tagged(_) => {
                let surface = seen
                    .surface(self.types, self.scratch)
                    .expect("a container or a tagged value opens");
                self.frames.push(Frame::Composite {
                    surface,
                    next: 0,
                    base,
                });
                return None;
            }
            Value::Number(number) => Value::Number(number),
            Value::Bool(flag) => Value::Bool(flag),
            Value::Null => Value::Null,
            Value::Str(source) => text(writer, source),
            Value::Type(type_value) => {
                self.adopt(type_value.handle());
                Value::Type(type_value.copied(writer))
            }
            Value::Knotted(_) | Value::Retyped(_) => {
                let member = value.knot_member().expect("a knot member");
                let retype = (member.ktype() != seen.ktype()).then(|| seen.ktype());
                match self.knots.get(&member.root()) {
                    Some(root) => self.retyped(root.sibling(member.index()), retype),
                    None => {
                        let held = self.held.len();
                        XF::held(&member, &mut |value| self.held.push(value));
                        self.frames.push(Frame::Knot {
                            member,
                            retype,
                            held,
                            end: self.held.len(),
                            next: 0,
                            base,
                        });
                        return None;
                    }
                }
            }
        };
        Some(finished)
    }

    /// A copied member, under a header at `retype` where it is seen at a type other than its own,
    /// that type adopted before the header is laid.
    fn retyped(
        &self,
        member: XF::Closed<'to>,
        retype: Option<DeclaredType<KType>>,
    ) -> Value<'to, XF::Closed<'to>> {
        match retype {
            Some(target) => {
                self.adopt(target);
                Value::Retyped(Retyped::of(self.writer, member, target))
            }
            None => Value::Knotted(member),
        }
    }

    /// The next child `frame` has not started, if any, its own memo adopted before the type it is
    /// seen at is read off it. A part its holder's type hides is never visited, so nothing of it is
    /// adopted.
    fn next_child(
        &self,
        frame: Frame<'x, 'from, XF::Closed<'from>>,
    ) -> Option<Seen<'from, XF::Closed<'from>>> {
        match frame {
            Frame::Composite { surface, next, .. } => (next < surface.len()).then(|| {
                self.adopt(surface.raw(next).ktype());
                surface.child(next, self.types, self.scratch)
            }),
            Frame::Knot {
                held, end, next, ..
            } => (held + next < end).then(|| {
                let value = self.held[held + next];
                self.adopt(value.ktype());
                Seen::of(value)
            }),
        }
    }

    /// Mark the top frame's next child started.
    fn advance(&mut self) {
        match self.frames.last_mut() {
            Some(Frame::Composite { next, .. } | Frame::Knot { next, .. }) => *next += 1,
            None => unreachable!("a child is started under a frame"),
        }
    }

    /// Lay down `frame`'s composite over its finished children, and pop them: a surface at the
    /// type it is seen at, weighed by what it lays down.
    fn finish(
        &mut self,
        frame: Frame<'x, 'from, XF::Closed<'from>>,
    ) -> Value<'to, XF::Closed<'to>> {
        let writer = self.writer;
        let (base, finished) = match frame {
            Frame::Composite { surface, base, .. } => {
                let built = &self.done[base..];
                let ktype = surface.ktype();
                let cells = || writer.fill(built.len(), |at| built[at]);
                let finished = match surface.parts() {
                    Parts::List { .. } => {
                        Value::List(List::weighed(writer, cells(), ktype, self.types))
                    }
                    Parts::Dict { .. } => {
                        let keys = writer.fill(surface.len(), |at| surface.key(at).rehomed(writer));
                        Value::Dict(Dict::weighed(writer, keys, cells(), ktype, self.types))
                    }
                    Parts::Record { .. } => {
                        let names = collect(writer, (0..surface.len()).map(|at| surface.name(at)));
                        Value::Record(Record::weighed(writer, names, cells(), ktype, self.types))
                    }
                    Parts::Tagged { .. } => {
                        Value::Tagged(Tagged::hold(writer, built[0], ktype, self.types))
                    }
                };
                (base, finished)
            }
            Frame::Knot {
                member,
                retype,
                held,
                end,
                base,
                ..
            } => {
                let mut answers = self.held[held..end].iter().zip(&self.done[base..]);
                let mut cursor = |asked: &Value<'from, XF::Closed<'from>>| {
                    let (listed, copy) = answers
                        .next()
                        .expect("copy_into asks for each held value once");
                    debug_assert!(same_word(listed, asked), "copy_into asks in held's order");
                    *copy
                };
                XF::carried_types(&member, &mut |kt| self.adopt(kt));
                let copied = XF::copy_into(writer, &member, &mut cursor);
                stamp_knot::<XF>(&copied, self.types);
                debug_assert!(
                    answers.next().is_none(),
                    "copy_into asks for every held value"
                );
                self.knots.insert(member.root(), copied.root());
                self.held.truncate(held);
                (base, self.retyped(copied, retype))
            }
        };
        self.done.truncate(base);
        finished
    }
}

/// Whether two value words are the same word: the same variant over the same scalar or the same
/// resident. Never walks a value.
fn same_word<X: Knotted>(a: &Value<'_, X>, b: &Value<'_, X>) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.to_bits() == b.to_bits(),
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Null, Value::Null) => true,
        (Value::Str(a), Value::Str(b)) => std::ptr::eq(*a, *b),
        (Value::Type(a), Value::Type(b)) => std::ptr::eq(*a, *b),
        (Value::List(a), Value::List(b)) => std::ptr::eq(*a, *b),
        (Value::Dict(a), Value::Dict(b)) => std::ptr::eq(*a, *b),
        (Value::Record(a), Value::Record(b)) => std::ptr::eq(*a, *b),
        (Value::Tagged(a), Value::Tagged(b)) => std::ptr::eq(*a, *b),
        (Value::Knotted(a), Value::Knotted(b)) => a == b,
        (Value::Retyped(a), Value::Retyped(b)) => std::ptr::eq(*a, *b),
        _ => false,
    }
}
