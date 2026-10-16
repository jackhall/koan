//! The memo bits a value door sets at construction: where every type memo a new value reaches
//! already resolves, read off its parts' bits and the chain it was built through.
//!
//! **root-closed** is set where every part is root-closed and the value's own memo lies in the
//! root. **settled** is set where every part meets the contract (either bit) and the chain
//! [interns where it lays][interns_where_it_lays], so every memo the door interned lands in the
//! root or in the branch of the half the value lies in. A clear bit is always safe: it only makes
//! an adoption walk further. An exit marks what it walked settled after adopting it
//! ([crossing.rs](crossing.rs)), and a restamp or a copy recomputes both through these folds. See
//! [design/type-tiers.md § An adoption stops at a settled part](../../design/type-tiers.md#an-adoption-stops-at-a-settled-part).

use crate::type_lattice::{DeclaredType, Habitat, KType};

use super::{Chain, Knotted, KnottedFamily, MemoBits, Value};

/// Whether every type `types` interns lands in the root or in the branch of the half a value door
/// beside it lays its value in: a chain over one half alone, paired to that half's own writer, or
/// the load's chain. A scratch chain reads a region tier, and a paired half lays in a tier the
/// value does not lie in, so neither does.
pub(crate) fn interns_where_it_lays(types: &Chain<'_, '_, '_, '_, '_>) -> bool {
    let halves = types.half();
    halves.region().is_none() && halves.half().interns_where_it_lays()
}

/// What a holder folds from one part. A leaf's and a type value's memos resolve in the root or in
/// the half a door over a chain that [interns where it lays][interns_where_it_lays] lays in, which
/// is the only place their `settled` answer is folded in. `probe` is `false` once the holder is
/// known not to be root-closed, so no root probe runs that cannot change its answer.
pub(crate) fn part_bits<X: Knotted>(
    part: &Value<'_, X>,
    types: &Chain<'_, '_, '_, '_, '_>,
    probe: bool,
) -> MemoBits {
    match part {
        Value::Number(_) | Value::Bool(_) | Value::Null | Value::Str(_) => MemoBits::BOTH,
        Value::Type(type_value) => MemoBits {
            root_closed: probe
                && types.in_root(type_value.ktype())
                && types.in_root(type_value.handle()),
            settled: true,
        },
        Value::List(list) => list.bits(),
        Value::Dict(dict) => dict.bits(),
        Value::Record(record) => record.bits(),
        Value::Tagged(tagged) => tagged.bits(),
        Value::Knotted(member) => member.bits(),
        Value::Retyped(header) => {
            let member = header.member().bits();
            MemoBits {
                root_closed: probe && member.root_closed && types.in_root(header.ktype()),
                settled: member.meets(),
            }
        }
    }
}

/// The bits of a composite under `ktype` over `parts`, laid beside `types`: root-closed where
/// every part is and `ktype` lies in the root, probed last; settled where every part meets the
/// contract and `types` interns where it lays.
pub(crate) fn composite_bits<'a, X: Knotted + 'a>(
    types: &Chain<'_, '_, '_, '_, '_>,
    ktype: KType,
    parts: impl IntoIterator<Item = &'a Value<'a, X>>,
) -> MemoBits {
    let mut folded = MemoBits {
        root_closed: true,
        settled: interns_where_it_lays(types),
    };
    for part in parts {
        let bits = part_bits(part, types, folded.root_closed);
        folded.root_closed &= bits.root_closed;
        folded.settled &= bits.meets();
        if !folded.meets() {
            return MemoBits::NONE;
        }
    }
    debug_assert!(
        !folded.settled || types.resolves(ktype),
        "a settled composite's own memo resolves beside it"
    );
    MemoBits {
        root_closed: folded.root_closed && types.in_root(ktype),
        settled: folded.settled,
    }
}

/// A restamp's bits: the old value's parts are the new one's, so the old bits answer for them,
/// and the new memo `ktype` is read off `types`.
pub(crate) fn restamp_bits(
    old: MemoBits,
    types: &Chain<'_, '_, '_, '_, '_>,
    ktype: impl Into<DeclaredType<KType>>,
) -> MemoBits {
    MemoBits {
        root_closed: old.root_closed && types.in_root(ktype.into()),
        settled: old.meets() && interns_where_it_lays(types),
    }
}

/// Stamp a newborn knot's bits on its root node: root-closed where every type its knot carries
/// lies in the root and every value it holds is root-closed; settled where every held value meets
/// the contract and `types`, the chain it was born through, interns where it lays.
pub(crate) fn stamp_knot<'graph, 'cell, XF: KnottedFamily<'graph>>(
    member: &XF::Closed<'cell>,
    types: &Chain<'_, '_, '_, '_, '_>,
) where
    'graph: 'cell,
{
    let mut bits = MemoBits {
        root_closed: true,
        settled: interns_where_it_lays(types),
    };
    XF::held(member, &mut |held| {
        if bits.meets() {
            let part = part_bits(&held, types, bits.root_closed);
            bits.root_closed &= part.root_closed;
            bits.settled &= part.meets();
        }
    });
    if bits.root_closed {
        XF::carried_types(member, &mut |kt| {
            bits.root_closed = bits.root_closed && types.in_root(kt);
        });
    }
    member.set_bits(bits);
}
