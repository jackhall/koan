//! The binary driver: one pairing policy, three instances.
//!
//! [`pairing`] is the whole table — which children of two nodes correspond, how variance turns
//! over between them, and what width verdict the arm carries. An instance supplies an entry guard,
//! a leaf verdict, a set-wise rule for unions, and a structural combine that reads the width
//! verdict generically. The order, the meet and the unifier's collector are the three.
//!
//! Two signatures and two quantified shapes reach the leaf verdict rather than a child pairing,
//! because their relations are schema-level and instantiation-level doors.

use smallvec::SmallVec;

use crate::parse::BinderSymbol;

use super::Variance;
use crate::type_lattice::handle::KType;
use crate::type_lattice::node::TypeNode;
use crate::type_lattice::record::Record;
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::shape::DispatchTokenElement;

/// What an arm's leftover children mean. The instance reads it rather than knowing which arm it is
/// looking at: the order turns each into an emptiness requirement, the meet into a keep-or-drop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Width {
    /// No names: every child pairs, and neither side can have a leftover.
    Positional,
    /// Records: the subtype has every field of the supertype, so `only_b` must be empty.
    ASupersetOfB,
    /// Function parameters: the subtype asks for no name the supertype does not, so `only_a` must
    /// be empty.
    ASubsetOfB,
    /// Constructor arguments: the two applications name the same parameters or nothing relates.
    Exact,
}

impl Width {
    /// Whether this arm's leftovers permit `a ≤ b`. The one reading of a width verdict, so an
    /// instance never has to know which arm it is looking at — and a walk asking the *reverse*
    /// question passes the leftovers the other way round.
    pub fn permits(self, only_a: &[Leftover], only_b: &[Leftover]) -> bool {
        match self {
            Width::Positional => true,
            Width::ASupersetOfB => only_b.is_empty(),
            Width::ASubsetOfB => only_a.is_empty(),
            Width::Exact => only_a.is_empty() && only_b.is_empty(),
        }
    }

    /// Whether a bound over this arm keeps the children only one side names. An **upper** bound
    /// keeps a leftover exactly where more children make a type more general — function parameters
    /// — and drops it where more children make it more specific — record fields. A lower bound is
    /// the mirror.
    pub fn bound_keeps_leftovers(self, upper: bool) -> bool {
        match self {
            Width::Positional | Width::Exact => false,
            Width::ASupersetOfB => !upper,
            Width::ASubsetOfB => upper,
        }
    }
}

/// A leftover child — one side's field that the other side does not name.
pub type Leftover = (BinderSymbol, KType);

/// What an instance implements to become a binary walk.
pub trait Lockstep {
    type Out;

    /// Run before the arm dispatch at every pair. `Some(out)` decides the pair outright — handle
    /// equality, a top or bottom, a fast path with nothing to solve.
    fn enter(&mut self, types: &TypeRegistry, a: KType, b: KType, v: Variance)
    -> Option<Self::Out>;

    /// The verdict for a pair the table does not relate structurally: two atoms, two signatures,
    /// two quantified shapes, or two arms of different shape.
    fn leaf(&mut self, types: &TypeRegistry, a: KType, b: KType, v: Variance) -> Self::Out;

    /// The verdict when either side is a union. A non-union side arrives as a one-element slice.
    fn set_wise(
        &mut self,
        types: &TypeRegistry,
        a: &[KType],
        b: &[KType],
        v: Variance,
        recurse: &mut dyn FnMut(&mut Self, KType, KType, Variance) -> Self::Out,
    ) -> Self::Out;

    /// The verdict for a structurally paired arm, from its children's verdicts and the arm itself.
    fn structural(&mut self, types: &TypeRegistry, paired: &[Self::Out], arm: Arm<'_>)
    -> Self::Out;

    /// Whether a child's verdict decides the whole arm, so the driver can stop pairing.
    fn short_circuits(&self, _out: &Self::Out) -> bool {
        false
    }
}

/// Walk `a` against `b` under `v` through `rules`.
pub fn lockstep<L: Lockstep>(
    types: &TypeRegistry,
    a: KType,
    b: KType,
    v: Variance,
    rules: &mut L,
) -> L::Out {
    if let Some(out) = rules.enter(types, a, b, v) {
        return out;
    }
    types.with_node(a, |na| {
        types.with_node(b, |nb| match pairing(na, nb, v) {
            Pairing::Leaf => rules.leaf(types, a, b, v),
            Pairing::SetWise => {
                let one = [a];
                let other = [b];
                let left = union_members(na).unwrap_or(&one);
                let right = union_members(nb).unwrap_or(&other);
                rules.set_wise(types, left, right, v, &mut |rules, x, y, v| {
                    lockstep(types, x, y, v, rules)
                })
            }
            Pairing::Structural {
                pairs,
                width,
                only_a,
                only_b,
                assembly,
            } => {
                let mut paired: SmallVec<[L::Out; 8]> = SmallVec::with_capacity(pairs.len());
                for (x, y, child_variance) in pairs {
                    let out = lockstep(types, x, y, child_variance, rules);
                    let stop = rules.short_circuits(&out);
                    paired.push(out);
                    if stop {
                        break;
                    }
                }
                rules.structural(
                    types,
                    &paired,
                    Arm {
                        width,
                        only_a: &only_a,
                        only_b: &only_b,
                        variance: v,
                        rebuild: Rebuilder {
                            types,
                            assembly,
                            operands: (a, b),
                        },
                    },
                )
            }
        })
    })
}

/// One structurally paired arm, as an instance reads it: what its children's verdicts mean, which
/// leftovers each side had, the polarity it was reached at, and the door back to a rebuilt node.
pub struct Arm<'n> {
    pub width: Width,
    pub only_a: &'n [Leftover],
    pub only_b: &'n [Leftover],
    pub variance: Variance,
    pub rebuild: Rebuilder<'n>,
}

/// How two nodes correspond.
///
/// The structural arm carries the whole pairing inline, so a walk over a compound node allocates
/// nothing; boxing it to even out the variants would put an allocation on the hot path of every
/// relation in the lattice.
#[allow(clippy::large_enum_variant)]
enum Pairing<'n> {
    Leaf,
    SetWise,
    Structural {
        pairs: SmallVec<[(KType, KType, Variance); 8]>,
        width: Width,
        only_a: SmallVec<[Leftover; 4]>,
        only_b: SmallVec<[Leftover; 4]>,
        assembly: Assembly<'n>,
    },
}

/// **The pairing table.** A new compound variant is a compile error here and in
/// [`Rebuilder::compose`], and nowhere else in this file.
fn pairing<'n>(a: &'n TypeNode, b: &'n TypeNode, v: Variance) -> Pairing<'n> {
    // A union on either side is set-wise, ahead of every structural arm: a union of records is not
    // a record, and pairing it as one would relate the wrong children.
    if matches!(a, TypeNode::Union { .. }) || matches!(b, TypeNode::Union { .. }) {
        return Pairing::SetWise;
    }
    match (a, b) {
        (TypeNode::List { element: x }, TypeNode::List { element: y }) => {
            positional([(*x, *y, v)].into_iter(), Assembly::List)
        }
        (TypeNode::Dict { key: xk, value: xv }, TypeNode::Dict { key: yk, value: yv }) => {
            positional([(*xk, *yk, v), (*xv, *yv, v)].into_iter(), Assembly::Dict)
        }
        (TypeNode::Record { fields: xf }, TypeNode::Record { fields: yf }) => {
            by_name(xf, yf, v, Width::ASupersetOfB, Assembly::Record)
        }
        (
            TypeNode::KFunction {
                params: xp,
                ret: xr,
            },
            TypeNode::KFunction {
                params: yp,
                ret: yr,
            },
        ) => {
            let mut paired = by_name(xp, yp, v.flipped(), Width::ASubsetOfB, Assembly::Function);
            if let Pairing::Structural { pairs, .. } = &mut paired {
                pairs.push((*xr, *yr, v));
            }
            paired
        }
        (
            TypeNode::ExpressionShape {
                quantifiers: xq,
                elements: xe,
                ret: xr,
            },
            TypeNode::ExpressionShape {
                quantifiers: yq,
                elements: ye,
                ret: yr,
            },
        ) => {
            // A quantified shape relates to another by instantiation, which is a leaf door, not a
            // child pairing.
            if !xq.is_empty() || !yq.is_empty() || xe.len() != ye.len() {
                return Pairing::Leaf;
            }
            let mut pairs: SmallVec<[(KType, KType, Variance); 8]> = SmallVec::new();
            for (x, y) in xe.iter().zip(ye.iter()) {
                match (x, y) {
                    (DispatchTokenElement::Slot(sx), DispatchTokenElement::Slot(sy)) => {
                        pairs.push((*sx, *sy, v.flipped()))
                    }
                    (DispatchTokenElement::Keyword(kx), DispatchTokenElement::Keyword(ky))
                        if kx == ky => {}
                    // A different key is a different bucket: two shapes under different keys have
                    // no common structure.
                    _ => return Pairing::Leaf,
                }
            }
            pairs.push((*xr, *yr, v));
            Pairing::Structural {
                pairs,
                width: Width::Positional,
                only_a: SmallVec::new(),
                only_b: SmallVec::new(),
                assembly: Assembly::Shape(xe),
            }
        }
        (
            TypeNode::ConstructorApply {
                constructor: xc,
                arguments: xa,
            },
            TypeNode::ConstructorApply {
                constructor: yc,
                arguments: ya,
            },
        ) => {
            let mut paired = by_name(xa, ya, v, Width::Exact, Assembly::Apply);
            if let Pairing::Structural { pairs, .. } = &mut paired {
                pairs.insert(0, (*xc, *yc, v));
            }
            paired
        }
        _ => Pairing::Leaf,
    }
}

/// A structural arm whose children pair by position and leave nothing over.
fn positional<'n>(
    pairs: impl Iterator<Item = (KType, KType, Variance)>,
    assembly: Assembly<'n>,
) -> Pairing<'n> {
    Pairing::Structural {
        pairs: pairs.collect(),
        width: Width::Positional,
        only_a: SmallVec::new(),
        only_b: SmallVec::new(),
        assembly,
    }
}

/// A structural arm whose children pair by field name, in `a`'s declaration order. The unmatched
/// fields of each side ride out as its leftovers, for the instance to read against `width`.
fn by_name<'n>(
    a: &Record<KType>,
    b: &Record<KType>,
    child: Variance,
    width: Width,
    assemble: fn(SmallVec<[BinderSymbol; 8]>) -> Assembly<'n>,
) -> Pairing<'n> {
    let mut pairs: SmallVec<[(KType, KType, Variance); 8]> = SmallVec::new();
    let mut keys: SmallVec<[BinderSymbol; 8]> = SmallVec::new();
    let mut only_a: SmallVec<[Leftover; 4]> = SmallVec::new();
    for (name, x) in a.iter() {
        match b.get(name.symbol()) {
            Some(y) => {
                pairs.push((*x, *y, child));
                keys.push(name);
            }
            None => only_a.push((name, *x)),
        }
    }
    let only_b: SmallVec<[Leftover; 4]> = b
        .iter()
        .filter(|(name, _)| a.get(name.symbol()).is_none())
        .map(|(name, y)| (name, *y))
        .collect();
    Pairing::Structural {
        pairs,
        width,
        only_a,
        only_b,
        assembly: assemble(keys),
    }
}

fn union_members(node: &TypeNode) -> Option<&[KType]> {
    match node {
        TypeNode::Union { members } => Some(members),
        _ => None,
    }
}

/// The shape of the arm being paired, so an instance that rebuilds can put one back together
/// without knowing which arm it is in.
enum Assembly<'n> {
    List,
    Dict,
    Record(SmallVec<[BinderSymbol; 8]>),
    Function(SmallVec<[BinderSymbol; 8]>),
    Shape(&'n [DispatchTokenElement]),
    Apply(SmallVec<[BinderSymbol; 8]>),
}

/// The door [`Lockstep::structural`] rebuilds its arm through. Handed to every instance; only the
/// ones whose `Out` is a type ever call it.
pub struct Rebuilder<'n> {
    types: &'n TypeRegistry,
    assembly: Assembly<'n>,
    operands: (KType, KType),
}

impl Rebuilder<'_> {
    /// The two handles this arm paired — what an instance falls back on when the arm's leftovers
    /// leave nothing to compose.
    pub fn operands(&self) -> (KType, KType) {
        self.operands
    }

    /// Rebuild this arm from its paired children, in the order the driver produced them, plus
    /// `extra` fields to carry over on a name-keyed arm. `extra` is ignored where the arm has no
    /// names to carry.
    pub fn compose(&self, paired: &[KType], extra: &[Leftover]) -> KType {
        let types = self.types;
        match &self.assembly {
            Assembly::List => types.list(paired[0]),
            Assembly::Dict => types.dict(paired[0], paired[1]),
            Assembly::Record(keys) => types.record(named(keys, paired, extra)),
            Assembly::Function(keys) => {
                let (values, ret) = paired.split_at(keys.len());
                types.function_type(named(keys, values, extra), ret[0])
            }
            Assembly::Shape(elements) => {
                let mut slots = paired.iter();
                let rebuilt: SmallVec<[DispatchTokenElement; 12]> = elements
                    .iter()
                    .map(|element| match element {
                        DispatchTokenElement::Slot(_) => DispatchTokenElement::Slot(
                            *slots.next().expect("one child per slot position"),
                        ),
                        keyword => *keyword,
                    })
                    .collect();
                let ret = *slots.next().expect("the return follows the slots");
                types.shape_type(&[], &rebuilt, ret).handle
            }
            Assembly::Apply(keys) => {
                types.constructor_apply(paired[0], named(keys, &paired[1..], extra))
            }
        }
    }
}

fn named(keys: &[BinderSymbol], values: &[KType], extra: &[Leftover]) -> Record<KType> {
    keys.iter()
        .copied()
        .zip(values.iter().copied())
        .chain(extra.iter().copied())
        .collect()
}
