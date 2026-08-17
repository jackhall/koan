//! [`WorkLabel`] — what a stuck slot renders as, and where that label comes from.
//!
//! The label is minted at install and read only by a drain that deadlocked, so these tests cover
//! the two halves separately: the classification a working expression produces, and the report line
//! each arm renders.

use super::*;
use crate::machine::core::{FrameStorageExt, program_storage, run_root_storage};
use crate::machine::model::{ExpressionPart, WorkingPart};
use crate::source::{SourceFile, Spanned};

/// A one-part run naming `name`, carrying `span` when given.
fn identifier_run<'a>(span: Option<Span>) -> Vec<Spanned<WorkingPart<'a>>> {
    let part = WorkingPart::Ast(ExpressionPart::Identifier("stuck"));
    vec![match span {
        Some(span) => Spanned::at(part, span),
        None => Spanned::bare(part),
    }]
}

/// **A node that knows where it came from labels by source.** The label holds the extent and the
/// file — both `Copy` — and nothing is rendered until the report asks.
#[test]
fn a_spanned_node_labels_by_source() {
    let program = program_storage();
    let region = run_root_storage();
    let _ = &program;
    let file = crate::source::register(SourceFile::new("deadlock.koan", "LET a = stuck\n".into()));
    let span = Span { start: 8, end: 13 };

    let expr = WorkingExpression::build(
        region.brand(),
        identifier_run(Some(span)),
        Some(span),
        Some(file),
    );

    assert!(
        matches!(WorkLabel::of(&expr), WorkLabel::Source { .. }),
        "a node carrying both an extent and a file labels by source",
    );
    assert_eq!(
        WorkLabel::of(&expr).render(),
        "deadlock.koan:1:9: stuck",
        "the source arm renders path:line:col plus the text the extent covers",
    );
}

/// **A run the machine assembled from no origin falls back to its dispatch shape.** The floor, not
/// the normal case: a synthesis with an origin carries that origin's file through
/// [`WorkingExpression::synthesized`] and lands in the `Source` arm instead.
#[test]
fn an_originless_run_labels_by_shape() {
    let program = program_storage();
    let region = run_root_storage();
    let _ = &program;

    let expr = WorkingExpression::new(region.brand(), identifier_run(None));

    let label = WorkLabel::of(&expr);
    assert!(
        matches!(label, WorkLabel::Shape(_)),
        "a run with no extent and no file has only its shape to report",
    );
    assert_eq!(
        label.render(),
        "<BareIdentifier>",
        "the shape arm renders the classification the expression already cached",
    );
}

/// **A synthesis takes its origin's file and the extent its own parts cover** — so an
/// operator-chain reduction or an extracted head still names the source it came from rather than
/// reporting as location-free.
#[test]
fn a_synthesized_run_inherits_its_origin() {
    let program = program_storage();
    let region = run_root_storage();
    let _ = &program;
    let file = crate::source::register(SourceFile::new("fold.koan", "a + b + c\n".into()));
    let origin = WorkingExpression::build(
        region.brand(),
        identifier_run(Some(Span { start: 0, end: 9 })),
        Some(Span { start: 0, end: 9 }),
        Some(file),
    );

    let reduced = WorkingExpression::synthesized(
        region.brand(),
        identifier_run(Some(Span { start: 0, end: 5 })),
        &origin,
    );

    assert_eq!(
        WorkLabel::of(&reduced).render(),
        "fold.koan:1:1: a + b",
        "the synthesized node names the origin's file over its own parts' extent",
    );
}

/// **A slot with no expression behind it reports a generic tag, not an empty sample.** A dep-finish
/// and a block fan-out both install through doors that carry no expression.
#[test]
fn a_slot_with_no_expression_reports_a_tag() {
    assert_eq!(WorkLabel::None.render(), "<wait>");
}
