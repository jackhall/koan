//! Working expressions: the AST's cache carried over, the splice path, synthesized extents, and
//! slot admission over every part kind.

use std::ptr;

use crate::parse::{BinderSymbol, ExpressionPart, KeyElement, PartClass, classify_dispatch_shape};
use crate::source::{Span, Spanned};
use crate::type_lattice::{KKind, KType};
use crate::values::{WorkingExpression, WorkingPart, admits, admits_part, part_ktype, text};

use super::{pin, with_fixture};

#[test]
fn a_working_copy_carries_the_parsed_cache() {
    with_fixture(|fixture| {
        let ast = fixture.parse("LET x = 1");
        fixture.in_cell(pin, |context| {
            let working = WorkingExpression::from_ast(context.writer(), &ast);
            assert_eq!(working.parts.len(), ast.parts.len());
            assert!(ptr::eq(working.stored_key(), ast.stored_key()));
            assert!(ast.cache().binder_plan_ref().is_some());
            assert!(ptr::eq(
                working.cache().binder_plan_ref().unwrap(),
                ast.cache().binder_plan_ref().unwrap()
            ));
            assert_eq!(working.binder_name_slot(), ast.binder_name_slot());
            assert_eq!(working.shape(), ast.shape());
            assert_eq!(working.span, ast.span);
            assert_eq!(working.file, ast.file);
            assert!(matches!(
                working.parts[1].value,
                WorkingPart::Ast(ExpressionPart::Identifier(_))
            ));
        })
    });
}

#[test]
fn a_splice_keeps_the_key_and_reads_the_head_again() {
    with_fixture(|fixture| {
        let ast = fixture.parse("(a) + 1");
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let working = WorkingExpression::from_ast(writer, &ast);
            let spliced = working.respliced(
                writer,
                working
                    .parts
                    .iter()
                    .enumerate()
                    .map(|(index, part)| match index {
                        0 => Spanned {
                            value: WorkingPart::Spliced {
                                value: text(writer, "a"),
                                from_name: None,
                            },
                            span: part.span,
                        },
                        _ => *part,
                    }),
            );
            assert!(ptr::eq(spliced.stored_key(), working.stored_key()));
            assert_eq!(
                spliced.shape(),
                classify_dispatch_shape(working.stored_key(), Some(PartClass::Spliced))
            );
            assert!(spliced.parts[0].value.as_value().is_some());
            assert_eq!(spliced.span, working.span);
        })
    });
}

#[test]
fn a_built_node_computes_its_key_and_a_synthesized_one_takes_its_origin() {
    with_fixture(|fixture| {
        let ast = fixture.parse("a + b");
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let origin = WorkingExpression::from_ast(writer, &ast);
            let spanned = |part, start, end| Spanned::at(part, Span { start, end });
            let parts = [
                spanned(origin.parts[0].value, 3, 4),
                spanned(origin.parts[1].value, 5, 6),
                spanned(WorkingPart::StagedSlot, 7, 9),
            ];
            let synthesized = WorkingExpression::synthesized(writer, &parts, &origin);
            assert_eq!(synthesized.span, Some(Span { start: 3, end: 9 }));
            assert_eq!(synthesized.file, origin.file);
            assert_eq!(synthesized.stored_key(), origin.stored_key());
            assert!(synthesized.binder_plan().is_none());
            let bare = [Spanned::bare(WorkingPart::StagedSlot)];
            let fallback = WorkingExpression::synthesized(writer, &bare, &origin);
            assert_eq!(fallback.span, origin.span);
            assert_eq!(fallback.stored_key(), [KeyElement::Slot]);
            assert!(fallback.in_type_context().under_type_sigil());
        })
    });
}

#[test]
fn admission_reads_each_part_kind() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let ast = fixture.parse("x 1");
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let working = WorkingExpression::from_ast(writer, &ast);
            let name = working.parts[0].value;
            let number = working.parts[1].value;
            assert!(admits(KType::IDENTIFIER, &name, types, scratch));
            assert!(!admits(KType::STR, &name, types, scratch));
            assert!(admits(KType::NUMBER, &number, types, scratch));
            let spliced = WorkingPart::Spliced {
                value: text(writer, "s"),
                from_name: BinderSymbol::declared("x", fixture.labels),
            };
            assert!(admits(KType::STR, &spliced, types, scratch));
            assert!(!admits(KType::IDENTIFIER, &spliced, types, scratch));
            let nested = WorkingPart::Expression(crate::values::resident(writer, working));
            for unfilled in [WorkingPart::StagedSlot, nested] {
                assert!(admits(KType::ANY, &unfilled, types, scratch));
                assert!(!admits(KType::KEXPRESSION, &unfilled, types, scratch));
            }
        })
    });
}

#[test]
fn every_part_shape_admits_the_type_it_reports() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let every_shape = fixture.parse(
            "x Number (a b) :(LIST OF Number) :{x :Number} [1 \"a\"] {\"k\": 1} {x = true} 1 \"s\" \
             true null #(a)",
        );
        assert_eq!(every_shape.parts.len(), 13);
        for part in every_shape.parts {
            let part = part.value;
            let reported =
                part_ktype(&part, types, scratch).expect("a non-keyword part fills a slot");
            assert!(
                admits_part(reported, &part, types),
                "{part:?} refuses its own type"
            );
        }
        assert!(admits_part(
            KType::of_kind(KKind::AnyType),
            &fixture.part("Number"),
            types
        ));
        assert!(!admits_part(
            KType::of_kind(KKind::Signature),
            &fixture.part("Number"),
            types
        ));
        assert!(!admits_part(
            types.list(KType::STR),
            &fixture.part("1"),
            types
        ));
    });
}
