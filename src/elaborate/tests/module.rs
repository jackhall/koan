//! A module's self-signature, over a module body activated in a cell with its slots bound by hand.

use crate::type_lattice::{KType, SchemaDraft, TypeNode};
use crate::values::{TypeValue, Value};

use super::{Program, nulls, with_program};

/// The signature whose value slots and manifest members are `values` and `types`.
fn signature(
    program: &Program<'_, '_, '_>,
    values: &[(&str, KType)],
    types: &[(&str, KType)],
) -> KType {
    let mut draft = SchemaDraft::new(program.scratch);
    for (name, handle) in values {
        let name =
            crate::parse::ValueSymbol::declared(name, program.labels).expect("an identifier");
        draft.insert_value_slot(name, *handle);
    }
    for (name, handle) in types {
        draft.insert_manifest(program.type_name(name), *handle);
    }
    program.types.signature(program.scratch, draft)
}

#[test]
fn a_module_reports_a_value_slot_per_value_binder_and_a_manifest_member_per_type_binder() {
    let source = "MODULE m = ((LET n = 1) (LET s = \"a\") (NEWTYPE Dist = Number))";
    with_program(
        source,
        |_, _, _| Vec::new(),
        nulls,
        |program| {
            let body = program.module_body("m");
            let dist = program.types.list(KType::NUMBER);
            program.bind_member(body, "n", Value::Number(1.0));
            program.bind_member(body, "s", crate::values::text(program.writer, "a"));
            program.bind_member(
                body,
                "Dist",
                Value::Type(TypeValue::new(program.writer, dist, program.types)),
            );
            let handle = crate::elaborate::self_signature(body, program.types, program.scratch)
                .expect("every slot is bound");
            assert_eq!(
                handle,
                signature(
                    &program,
                    &[("n", KType::NUMBER), ("s", KType::STR)],
                    &[("Dist", dist)],
                ),
            );
            let TypeNode::Signature { schema, .. } = program.types.node(handle) else {
                panic!("a self-signature is a Signature node");
            };
            assert!(
                schema.abstract_members.is_empty(),
                "a module's signature declares no abstract member",
            );
            assert!(schema.keyworded.is_empty() && schema.operators.is_empty());
        },
    );
}

#[test]
fn an_empty_module_body_is_the_empty_signature() {
    with_program(
        "MODULE m = (1)",
        |_, _, _| Vec::new(),
        nulls,
        |program| {
            let body = program.module_body("m");
            assert_eq!(
                crate::elaborate::self_signature(body, program.types, program.scratch),
                Ok(KType::EMPTY_SIGNATURE),
            );
        },
    );
}

#[test]
fn two_modules_binding_the_same_members_in_either_order_are_one_handle() {
    let write = |source: &'static str| {
        with_program(
            source,
            |_, _, _| Vec::new(),
            nulls,
            |program| {
                let body = program.module_body("m");
                program.bind_member(body, "a", Value::Number(1.0));
                program.bind_member(body, "b", crate::values::text(program.writer, "x"));
                crate::elaborate::self_signature(body, program.types, program.scratch)
                    .expect("every slot is bound")
            },
        )
    };
    assert_eq!(
        write("MODULE m = ((LET a = 1) (LET b = \"x\"))"),
        write("MODULE m = ((LET b = \"x\") (LET a = 1))"),
    );
}

#[test]
fn a_slot_still_claimed_by_its_binder_leaves_the_module_unsigned() {
    let source = "MODULE m = ((LET n = 1) (LET s = \"a\"))";
    with_program(
        source,
        |_, _, _| Vec::new(),
        nulls,
        |program| {
            let body = program.module_body("m");
            program.bind_member(body, "n", Value::Number(1.0));
            let refused = crate::elaborate::self_signature(body, program.types, program.scratch)
                .expect_err("`s` is still claimed");
            assert_eq!(refused.binder, program.binder);
            assert_eq!(
                refused.name,
                crate::parse::BinderSymbol::classify("s").unwrap()
            );
        },
    );
}

#[test]
fn a_module_member_carries_the_type_its_value_carries_not_one_walked_from_its_contents() {
    let source = "MODULE m = (LET xs = 1)";
    with_program(
        source,
        |_, _, _| Vec::new(),
        nulls,
        |program| {
            let body = program.module_body("m");
            // A list stamped with a wider element type reports that type, not its cells'.
            let list = crate::values::List::new(
                program.writer,
                [Value::Number(1.0)].into_iter(),
                program.types,
                program.scratch,
            )
            .with_type(program.writer, program.types.list(KType::ANY));
            program.bind_member(body, "xs", Value::List(list));
            let handle = crate::elaborate::self_signature(body, program.types, program.scratch)
                .expect("every slot is bound");
            assert_eq!(
                handle,
                signature(&program, &[("xs", program.types.list(KType::ANY))], &[]),
            );
        },
    );
}
