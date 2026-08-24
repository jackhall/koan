use super::{WorkingExpression, WorkingPart};
use crate::builtins::test_support::{parse_one, probe_symbol};
use crate::machine::core::program_storage;
use crate::machine::model::ast::ExpressionPart;
use crate::source::Spanned;

/// The splice door inherits the structural cache instead of rebuilding it: the bucket key comes
/// back as the very run construction bumped — `ptr::eq`, not merely equal — and the operator probe
/// rides through with it. A rebuild would bump a duplicate key run per reduction step, and a chain
/// splices once per step.
#[test]
fn a_resplice_inherits_the_key_run_and_the_operator_probe() {
    let program = program_storage();
    let brand = program.brand().region();
    let chain = WorkingExpression::from_ast(
        brand,
        parse_one(
            &program,
            &crate::machine::model::LabelInterner::new(),
            "a + b * c",
        ),
    );
    let probe =
        crate::machine::model::KeywordSymbol::of_run(&[probe_symbol("*"), probe_symbol("+")]);
    assert_eq!(chain.operator_probe(), Some(probe));

    // The splice shape: every operand slot gives way to a staging hole, every keyword position
    // stands.
    let respliced = chain.respliced(
        brand,
        chain.parts.iter().map(|part| Spanned {
            value: match part.value {
                WorkingPart::Ast(ExpressionPart::Keyword(_)) => part.value,
                _ => WorkingPart::StagedSlot,
            },
            span: part.span,
        }),
    );

    assert!(
        std::ptr::eq(chain.stored_key(), respliced.stored_key()),
        "a splice writes no keyword position, so the key run is the same allocation",
    );
    assert_eq!(chain.operator_probe(), respliced.operator_probe());
    assert_eq!(chain.untyped_key(), respliced.untyped_key());
}
