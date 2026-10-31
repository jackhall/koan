//! Two member reads by their tokens alone, no reader asked what the owner holds.
//!
//! A path is a member read whose label is a type token, read through a chain of value members
//! rooted at a value name or at an expression written in place — `m.Carrier`, `(PICK p).Set`. The
//! shape builder records each one a type spells ([`BodyShape::path_spellings`]), so the type
//! channel mints its entry however far the elaborator reads the type holding it; the elaborator
//! decides a path by the same tokens, beside its root's mention.
//!
//! A tag projection is a member read labelled by a type token off a type — `Maybe.Some`,
//! `Shape.Sized.Some`, `m.Mb.Some`. The shape builder records the owner of one heading a
//! construction, and the load reads the union the construction is reached through off it.
//!
//! [`BodyShape::path_spellings`]: super::BodyShape::path_spellings

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::{BuiltinShapeId, ExpressionPart, KExpression};
use crate::symbols::{TypeSymbol, ValueSymbol};

/// A path's spelling: its root, the value members read from it in written order, and its label.
pub(crate) struct PathSpelling<'graph, 'x> {
    pub(crate) root: SpelledRoot<'graph>,
    pub(crate) chain: BumpVec<'x, ValueSymbol>,
    pub(crate) label: TypeSymbol,
}

/// Where a path is rooted: a value name, read through its mention, or an expression written in
/// place, whose module the path reads.
#[derive(Clone, Copy)]
pub(crate) enum SpelledRoot<'graph> {
    Name(&'graph ExpressionPart<'graph>),
    Inline(&'graph ExpressionPart<'graph>),
}

/// `node` as a path by its tokens alone: a member read labelled by a type token, through value
/// member reads, down to a value name or to any other expression, written in place. `None` for any
/// other node.
pub(crate) fn path_spelling<'graph, 'x>(
    node: &KExpression<'graph>,
    scratch: BumpAllocator<'x>,
) -> Option<PathSpelling<'graph, 'x>> {
    let attribute = |node: &KExpression<'graph>| match (node.cache().builtin_shape(), node.parts) {
        (Some(form), [_, owner, label]) if form.id == BuiltinShapeId::Attribute => {
            Some((&owner.value, &label.value))
        }
        _ => None,
    };
    let (mut root, ExpressionPart::Type(label)) = attribute(node)? else {
        return None;
    };
    let mut chain = BumpVec::new_in(scratch);
    while let ExpressionPart::Expression(node) = root {
        if let Some((owner, step)) = attribute(node) {
            let ExpressionPart::Identifier(step) = step else {
                return None;
            };
            chain.push(*step);
            root = owner;
            continue;
        }
        match node.parts {
            [only] if node.cache().builtin_shape().is_none() => root = &only.value,
            _ => break,
        }
    }
    let root = match root {
        ExpressionPart::Identifier(_) => SpelledRoot::Name(root),
        ExpressionPart::Expression(_) => SpelledRoot::Inline(root),
        _ => return None,
    };
    chain.reverse();
    Some(PathSpelling {
        root,
        chain,
        label: *label,
    })
}

/// `node` as a tag projection by its tokens alone: a member read labelled by a type token off a
/// type — a type name, or a type expression such as another projection or a path — so
/// `Maybe.Some`, `Shape.Sized.Some` and `m.Mb.Some`. Its owner part and label; `None` for any
/// other node, a module's type member `m.Mb` among them.
pub(crate) fn tag_projection<'graph>(
    node: &'graph KExpression<'graph>,
) -> Option<(&'graph ExpressionPart<'graph>, TypeSymbol)> {
    match (node.cache().builtin_shape(), node.parts) {
        (Some(form), [_, owner, label]) if form.id == BuiltinShapeId::Attribute => {
            match (&owner.value, &label.value) {
                (
                    ExpressionPart::Type(_) | ExpressionPart::SigiledTypeExpr(_),
                    ExpressionPart::Type(label),
                ) => Some((&owner.value, *label)),
                _ => None,
            }
        }
        _ => None,
    }
}
