//! What the machine does with a binder: the names it fixes itself, the refusal it renders, and the
//! declaration pre-scan a module body runs.
//!
//! The structural reading — which form is a binder, what name and bucket keys it declares — is the
//! parser's, in [`crate::parse::forms::binder`].

pub(crate) mod signature;

use crate::machine::core::{KError, KErrorKind, body_statement_refs};
use crate::machine::model::registries::RunRegistries;
use crate::machine::model::types::{AnnouncedData, display_label, pair_list_names};
use crate::parse::ast::KExpression;
pub(crate) use crate::parse::forms::binder::union_schema;

use crate::parse::forms::binder::{BinderSurface, SymbolError};
use crate::parse::labels::{LabelInterner, StaticName, ValueSymbol};

/// The names the machine itself fixes in Rust source for binders no program spells a declaration
/// for. Each declares as a [`StaticName`] and is minted once for the process, so a form binds by
/// loading the symbol rather than by classifying the spelling again per evaluation.
///
/// They live here, beside the extractors, because they answer the same question the form table does
/// — what a form binds — for the forms whose binder is implicit in the surface rather than written
/// in it. The builtins that install them read them back from here, so there is one spelling of
/// each.
pub(crate) struct MachineBinders {
    /// What every `MATCH` and `TRY` arm binds its scrutinee under.
    pub(crate) arm: StaticName<ValueSymbol>,
    /// The binary `OP` body's two operands, named by the surface rather than declared as
    /// parameters.
    pub(crate) operand_left: StaticName<ValueSymbol>,
    pub(crate) operand_right: StaticName<ValueSymbol>,
    /// The unary `OP` body's single parameter: the whole operand run as one list.
    pub(crate) operands: StaticName<ValueSymbol>,
    /// What a `_` slot in an expression-shape head binds under. A shape's slots are positional and
    /// its type drops their names, so the head needs a binder only to ride the shared signature
    /// parse; nothing reads this one back.
    pub(crate) slot: StaticName<ValueSymbol>,
}

pub(crate) static MACHINE_BINDERS: MachineBinders = MachineBinders {
    arm: crate::static_name!(ValueSymbol, "it"),
    operand_left: crate::static_name!(ValueSymbol, "left"),
    operand_right: crate::static_name!(ValueSymbol, "right"),
    operands: crate::static_name!(ValueSymbol, "operands"),
    slot: crate::static_name!(ValueSymbol, "slot"),
};

impl SymbolError {
    /// The diagnostic, rendered against the run that interned the glyph.
    pub(crate) fn into_error(self, labels: &LabelInterner) -> KError {
        KError::new(KErrorKind::ShapeError(match self {
            SymbolError::Shape => {
                "operator symbol must be one quoted token: `OP #(+) OVER Number = (…)`".to_string()
            }
            SymbolError::Reserved(symbol) => format!(
                "`{}` is reserved by the operator-declaration surface and cannot name an operator",
                labels.display(symbol.symbol()),
            ),
        }))
    }
}

/// The nominal-type declaration surfaces a module body pre-announces, as
/// [`announced_type_declaration`] classifies them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeDeclarationSurface {
    NewType,
    Union,
}

/// What `expression` announces to its module body's declaration window, or `None` if it announces
/// nothing.
///
/// Recognition is by the node's cached [`FORMS`](crate::parse::forms::FORMS) entry —
/// a full bucket key, every keyword pinned in position — so a user overload that merely shares a
/// head keyword announces nothing, and the constructor-family key `NEWTYPE <decl>` is excluded
/// structurally rather than by inspecting what its extractor would return. Only a statement at the
/// body's top level is offered here; a declaration nested inside another statement's slot keeps
/// ordinary dataflow order.
pub(crate) fn announced_type_declaration(
    expression: &KExpression<'_>,
) -> Option<TypeDeclarationSurface> {
    match expression.cache().form()?.binder?.surface {
        BinderSurface::NewTypeDef => Some(TypeDeclarationSurface::NewType),
        BinderSurface::UnionDef => Some(TypeDeclarationSurface::Union),
        BinderSurface::OperatorDef | BinderSurface::Other => None,
    }
}

/// Pre-scan `body`'s **top-level** statements for the type declarations the body announces, so
/// every one of their names is visible to every statement regardless of order — which is what lets
/// a plain module host a mutually-recursive group.
///
/// A `NEWTYPE` announces one standalone member. A `UNION` announces one member per statically
/// scannable variant tag, each **owned** by the union's binder: a variant is never
/// bare-name-resolvable and never lands in `bindings.types`, so it is reached only through the
/// binder or by member projection off it (`:(Tree.Node)`). A `UNION` whose schema does not scan announces nothing at all —
/// its own dispatch surfaces the real diagnostic.
///
/// Nested and computed declarations are untouched by construction: the scan sees only the statement
/// split [`body_statement_refs`] draws, the same boundary `GROUP` reads its members off.
pub(crate) fn announce_type_members(
    body: &KExpression<'_>,
    module: ValueSymbol,
    registries: &RunRegistries,
) -> Result<Option<AnnouncedData>, KError> {
    let mut announced = AnnouncedData::default();
    for statement in body_statement_refs(body) {
        let Some(surface) = announced_type_declaration(statement) else {
            continue;
        };
        let Some(name) = statement.binder_name_from_type_part() else {
            continue;
        };
        // The parser classified and interned the binder token, so the window, the members it
        // seals and every diagnostic naming one already share one currency.
        let binder = name;
        if announced.declares(binder) || announced.binds(binder) {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "module `{}` declares type `{}` twice",
                display_label(module.symbol(), registries),
                display_label(binder.symbol(), registries),
            ))));
        }
        match surface {
            TypeDeclarationSurface::NewType => {
                announced.announce(binder);
            }
            TypeDeclarationSurface::Union => {
                // The variant tags are the union's announced members. A schema this scan cannot
                // read is left entirely unannounced rather than half-announced.
                let Some(schema) = union_schema(statement) else {
                    continue;
                };
                match pair_list_names(&schema, "UNION schema", registries) {
                    Ok(tags) => {
                        announced.announce_binder(binder, tags);
                    }
                    Err(_) => continue,
                }
            }
        }
    }
    Ok((!announced.is_empty()).then_some(announced))
}
