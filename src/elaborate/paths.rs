//! What a **path** — a type member read through a module-valued expression — is where it runs, and
//! what the load records to compute it ([design/modules.md § Carriers and paths](../../design/modules.md#carriers-and-paths)).
//!
//! A path through a root whose declared type is an application naming the path's label a head
//! parameter is what the module fits that application at: the solution *fits* gives, as the view
//! door's does — the pin, where the application pins the label. Every other path reads the
//! module's own type member. The load records
//! the application once per path, as [`declared_through`] reads it off the root's declared type;
//! the run binds a path variable through [`path_binding`], and the run's elaborator reads a path
//! written in a type it left unknown through the same function.

use crate::memory::{Bump, BumpAllocator, BumpVec};
use crate::scope::{ActivationView, Coordinate, PathEntry, solutions};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, KType, Parametric, TypeNode, fits_application, member, substitute_levels,
};
use crate::values::{Chain, KnottedFamily};

use super::members::{SignatureMember, schema_of, signature_member};

/// The application a path is read through: `declared`, a root's declared type, walked through the
/// value members `chain` names, where the walk ends at an application of a signature naming
/// `label` a head parameter. `None` where it ends anywhere else, and the path reads the module's
/// own type member.
pub fn declared_through(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    declared: Parametric,
    chain: &[ValueSymbol],
    label: TypeSymbol,
) -> Option<Parametric> {
    let mut held = declared;
    for step in chain {
        match signature_member(types, scratch, held, BinderSymbol::Value(*step))? {
            SignatureMember {
                declared: DeclaredType::Type(inner),
                unpinned: None,
            } => held = inner,
            _ => return None,
        }
    }
    let signature = match types.node(held) {
        TypeNode::Signature { .. } => types.concrete(held)?,
        TypeNode::SignatureApply { signature, .. } => signature,
        _ => return None,
    };
    let schema = schema_of(signature, types)?;
    member(schema.parameters, label).is_some().then_some(held)
}

/// The type a path binds to where it runs: through the module of signature `held` — its root's —
/// walked through the value members `chain` names, the solution for `label` where the module fits
/// `through`, and otherwise the module's own type member `label`. `None` where a step names no
/// module member, or the module does not fit or names no such member.
pub fn path_binding(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    held: KType,
    chain: &[ValueSymbol],
    label: TypeSymbol,
    through: Option<KType>,
) -> Option<KType> {
    let inner = member_module(types, scratch, held, chain)?;
    let Some(through) = through else {
        return member_type(types, scratch, inner, BinderSymbol::Type(label));
    };
    let mut pins = BumpVec::new_in(scratch);
    let signature = match types.node(through) {
        TypeNode::SignatureApply {
            signature,
            pins: pinned,
        } => {
            pins.extend(pinned.iter());
            signature
        }
        _ => through,
    };
    let solution = fits_application(types, scratch, inner, signature, &pins).ok()?;
    types.concrete(member(solution, label)?)
}

/// The type a path entry binds to, read through the module at `at` of `view`: its `through`
/// solved where `view`'s shape locates its variables, then [`path_binding`] — or, where the load
/// resolved it closed, that type. What [`solutions`] is handed to bind a path variable.
pub fn bound_path<'graph, XF: KnottedFamily<'graph>>(
    view: &ActivationView<'graph, '_, XF>,
    types: &Chain<'_, '_, '_, '_>,
    at: Coordinate,
    entry: &PathEntry<'graph>,
) -> Option<KType> {
    if let Some(closed) = entry.closed() {
        return Some(closed);
    }
    let held = view.read(at);
    held.as_module()?;
    let scratch = Bump::new();
    let through = match entry.through {
        None => None,
        Some(through) => Some(match types.concrete(through) {
            Some(closed) => closed,
            None => {
                let located = view.shape().through_at(entry.level)?;
                let bindings = solutions(located, view, &scratch, &|_, _| None)?;
                types.concrete(substitute_levels(types, &scratch, through, &bindings))?
            }
        }),
    };
    path_binding(
        types,
        &scratch,
        held.concrete_ktype(),
        entry.chain,
        entry.label,
        through,
    )
}

/// The signature of the module a module of signature `signature` holds through the value members
/// `chain` names, each step's read off the one before it. `None` where a step names no member at
/// a type, or names a head parameter its application leaves unpinned.
pub(super) fn member_module(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    signature: KType,
    chain: &[ValueSymbol],
) -> Option<KType> {
    let mut held = signature;
    for step in chain {
        held = member_type(types, scratch, held, BinderSymbol::Value(*step))?;
    }
    Some(held)
}

/// The member `name` of `of` read at a type, every head parameter it names pinned.
pub(super) fn member_type(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    of: KType,
    name: BinderSymbol,
) -> Option<KType> {
    match signature_member(types, scratch, of.into(), name) {
        Some(SignatureMember {
            declared: DeclaredType::Type(declared),
            unpinned: None,
        }) => types.concrete(declared),
        _ => None,
    }
}
