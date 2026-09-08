//! The `FOR ALL (<names>)` group a quantified head declares.
//!
//! The group is captured raw, like the head it precedes, so its names are read here rather than
//! resolved by the dispatch lane — they name nothing yet. Each becomes the `Quantified(index)`
//! leaf at its position in the group, bound in a child scope the head then elaborates against,
//! so `x :Elt` and `:(Elt AS Wrap)` lower structurally through the unchanged paths.

use smallvec::SmallVec;

use crate::machine::core::RegionBrand;
use crate::machine::model::labels::TypeSymbol;
use crate::machine::model::{ExpressionPart, KExpression, RunRegistries, render_label};
use crate::machine::{KError, KErrorKind, Scope};

use super::finalize::Quantification;

/// Read a `(<names>)` group into the quantifier list it declares. Every part must be a bare Type
/// token: a quantifier is a name the call solves, so there is nothing else it could be.
fn parse_group<'a>(
    group: &KExpression<'_>,
    brand: RegionBrand<'a>,
    registries: &RunRegistries,
) -> Result<&'a [TypeSymbol], KError> {
    let shape = |message: String| KError::new(KErrorKind::ShapeError(message));
    if group.parts.is_empty() {
        return Err(shape(
            "a `FOR ALL` group names at least one quantifier — drop the group to declare an \
             unquantified shape"
                .to_string(),
        ));
    }
    // Staged on the stack: the run is validated before it is homed, and a group names a handful of
    // quantifiers, so the inline capacity covers every real one.
    let mut names: SmallVec<[TypeSymbol; 4]> = SmallVec::with_capacity(group.parts.len());
    for part in group.parts {
        let ExpressionPart::Type(name) = part.value else {
            return Err(shape(format!(
                "a `FOR ALL` group names quantifiers with Type tokens, but `{}` is not one",
                part.value.summary(&registries.labels),
            )));
        };
        if names.contains(&name) {
            return Err(shape(format!(
                "`FOR ALL` names `{}` twice; each quantifier is one name the call solves",
                render_label(name.symbol(), registries),
            )));
        }
        names.push(name);
    }
    Ok(brand.allocator().slice_from_iter(names))
}

/// Read the group and bind it: the names in `Quantified(index)` order, and the child scope of
/// `ctx.scope` they stand in.
pub(crate) fn read_quantification<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
    group: &KExpression<'_>,
) -> Result<Quantification<'a>, KError> {
    let names = parse_group(group, ctx.scope.brand(), ctx.registries)?;
    let scope: &'a Scope<'a> = ctx.scope.alloc_child_binding_types(
        names
            .iter()
            .enumerate()
            .map(|(index, name)| (*name, ctx.types().quantified(index))),
        ctx.registries,
    )?;
    Ok(Quantification { names, scope })
}
