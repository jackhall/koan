//! What a call seeks.
//!
//! A call hands the drain a [`Seek`], whatever its frame's placement: the arena node the callee was
//! born under and, where the callee's body keys a settlement — the load laid it a key list
//! (`LoadFacts::settlement_keys`) — the key of its segment under the content at each keyed name and
//! the image of the types that content carries. A parameter's content is its argument; a `FOR ALL`
//! name's is the call's solution at it, digested as the type value the frame binds: carried by a
//! keyworded call, held by an instance, and solved here, on the asker's region, for a call by
//! name. A call past a header or a barrier of a body keyed on a `FOR ALL` name seeks no
//! settlement, since its frame solves only once the arguments are retyped or have crossed, unless
//! it calls an instance, whose solution holds wherever it is called from. See
//! [build/README.md](../build/README.md).

use crate::build::key_of;
use crate::knot::{Function, KValue, Knotted};
use crate::memory::{Bump, BumpVec};
use crate::scheduler::Seek;
use crate::scope::{Code, Facts};
use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;
use crate::values::digest::type_digest;
use crate::values::{ArenaRef, BindingImage, Chain, KeyedTypes};

use super::body::admitted;
use super::bundle::KBirth;
use super::record::CallKind;

/// The function a call of `callee` runs, behind every barrier: `None` for a callee that is no
/// function.
pub fn called_function<'graph, 'cell>(
    callee: KValue<'graph, 'cell>,
) -> Option<&'cell Function<'graph, 'cell, Knotted<'graph, 'cell>>> {
    callee.as_callable()?.behind_barriers().function()
}

/// What the call `birth` seeks: the arena node its callee was born under — the root for a birth
/// that is no call or a callee that is no function — and the settlement under it its body keys,
/// if the asker can read its content at every keyed name. `types` is the asker's evaluation
/// chain, which reads the arguments; `region` its region's, where a call by name's solution is
/// solved.
pub fn seek<'graph>(
    birth: &KBirth<'graph, '_>,
    code: &Code<'_>,
    facts: &Facts<'_>,
    types: &Chain<'_, '_, '_, '_>,
    region: &Chain<'_, '_, '_, '_>,
) -> Seek<'graph> {
    let parent = match birth {
        KBirth::Call { callee, .. } => {
            called_function(*callee).map_or(ArenaRef::ROOT, |function| function.closure().build())
        }
        _ => ArenaRef::ROOT,
    };
    Seek {
        parent,
        settlement: settlement(birth, code, facts, types, region),
    }
}

/// The key and binding image of the settlement the call `birth` seeks under its callee's arena
/// node: `None` for a birth that is no call, a callee that is no function, a body that keys no
/// settlement, and a call whose content at a keyed name the asker cannot read.
fn settlement<'graph>(
    birth: &KBirth<'graph, '_>,
    code: &Code<'_>,
    facts: &Facts<'_>,
    types: &Chain<'_, '_, '_, '_>,
    region: &Chain<'_, '_, '_, '_>,
) -> Option<(crate::values::SettlementKey, BindingImage<'graph>)> {
    let KBirth::Call {
        program,
        callee,
        arguments,
        kind,
        contributed,
        ..
    } = birth
    else {
        return None;
    };
    let (callee, arguments) = (*callee, *arguments);
    let function = called_function(callee)?;
    let shape = code.shape(function.body().segment);
    let keyed = facts.settlement_keys(shape)?;
    let scratch = Bump::new();
    // The call's solution of its group, solved at most once and only for a keyed `FOR ALL` name:
    // `None` past a header or a barrier, whose frame solves after retyping or crossing.
    let solve = || -> Option<BumpVec<'_, KType>> {
        let mut solved = BumpVec::new_in(&scratch);
        if let Some(instance) = function.instance() {
            solved.extend_from_slice(instance);
            return Some(solved);
        }
        let node = callee.as_callable()?;
        if node.coerced().is_some() || callee.ktype() != node.declared() {
            return None;
        }
        match kind {
            CallKind::Keyworded => {
                solved.resize(function.quantifier_map().iter().count(), KType::NEVER);
                for (name, index) in function.quantifier_map().iter() {
                    let field = arguments.field(name.symbol(), types, &scratch)?;
                    solved[index] = field.value().as_type()?.handle();
                }
                Some(solved)
            }
            CallKind::ByName => {
                let view = node.declared().view(region);
                admitted(region, &scratch, view, arguments, contributed).ok()?
            }
        }
    };
    let mut solution = None;
    let mut digests = BumpVec::with_capacity_in(keyed.len(), &scratch);
    let mut binding = smallvec::SmallVec::new();
    for slot in keyed {
        let name = shape.slot_name(*slot);
        if let BinderSymbol::Type(name) = name
            && let Some(index) = function.quantifier_index(name)
        {
            if solution.is_none() {
                solution = Some(solve()?);
            }
            let solved = *solution.as_ref()?.get(index)?;
            let kind = KType::of_kind(solved.kind_of(region));
            digests.push(type_digest(solved, kind));
            binding.push(KeyedTypes {
                carried: kind.into(),
                named: Some(solved),
            });
            continue;
        }
        let argument = arguments.field(name.symbol(), types, &scratch)?.value();
        digests.push(argument.digest(types, &scratch));
        binding.push(KeyedTypes::of(&argument));
    }
    let key = key_of(shape.segment(), digests.into_iter());
    Some((key, BindingImage::of(binding, program.tiers(), types)))
}
