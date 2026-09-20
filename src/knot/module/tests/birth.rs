//! A module member born through the tie: what its node holds, in what order, and what refuses it.

use std::ptr;

use crate::scope::{Binding, ShapeKind, Slot};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, TypeNode};
use crate::values::{Incomparable, Knotted as _, Resolved, Value};

use crate::knot::tests::{Fixture, bound, callable, declared, pin, read, with_fixture};
use crate::knot::{KActivation, Knotted, Supplied, Untieable, tie};

use super::super::body_activation;

/// The module bound under `name`, and its node.
fn module<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    activation: &KActivation<'graph, 'cell>,
    name: &str,
) -> Knotted<'graph, 'cell> {
    bound(fixture, activation, name)
        .as_module()
        .unwrap_or_else(|| panic!("`{name}` is bound to a module"))
}

#[test]
fn a_module_node_holds_its_signature_and_its_members_in_layout_order() {
    let source = "\
MODULE m = ((LET zero = 0) (LET name = \"m\") (NEWTYPE Dist = Number))
LET outside = 1";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        fixture.in_cell(pin, |context, binder| {
            let activation = fixture.run(context.writer(), &lines, binder, &[]);
            let m = module(fixture, activation, "m");
            assert!(matches!(m.resolve(), Resolved::Module));
            assert_eq!(
                Value::Knotted(m).as_callable(),
                None,
                "a module is not callable"
            );
            assert_eq!(
                m.member().knot().len(),
                1,
                "every module is a one-node knot"
            );

            // The members are the body's slots in slot order: value names first, then type names,
            // each channel sorted by name.
            let node = m.module().expect("a module node");
            let body = activation
                .shape()
                .births(activation.shape().slot(fixture.name("m")).unwrap().0)
                .expect("a module binder births its body");
            assert_eq!(body.kind(), ShapeKind::Module);
            assert_eq!(node.members().len(), body.slots());
            let names: Vec<BinderSymbol> = (0..body.slots())
                .map(|slot| body.slot_name(Slot(slot as u32)))
                .collect();
            let values: Vec<_> = names
                .iter()
                .filter_map(|name| match name {
                    BinderSymbol::Value(name) => Some(*name),
                    BinderSymbol::Type(_) => None,
                })
                .collect();
            assert_eq!(values.len(), 2, "the two value names take the first slots");
            assert!(values.is_sorted(), "the value channel is sorted by name");
            assert!(
                matches!(names[2], BinderSymbol::Type(_)),
                "the type names take the slots after",
            );
            let member = |name: &str| {
                let (slot, _) = body.slot(fixture.name(name)).expect("a declared member");
                node.members()[slot.index()]
            };
            assert!(matches!(member("zero"), Value::Number(n) if n == 0.0));
            assert_eq!(member("name").as_str(), Some("m"));
            let Value::Type(dist) = member("Dist") else {
                panic!("a type member is a type value");
            };

            // The signature is the memo, and reports each member at the type its value carries.
            let TypeNode::Signature { schema, .. } = fixture.types.node(m.ktype()) else {
                panic!("a module's type is a Signature node");
            };
            assert!(schema.abstract_members.is_empty());
            assert_eq!(schema.value_slots.len(), 2);
            assert_eq!(schema.manifest_members.len(), 1);
            assert_eq!(schema.manifest_members[0].1, dist.handle());
            assert_eq!(
                crate::type_lattice::member(
                    schema.value_slots,
                    match fixture.name("zero") {
                        BinderSymbol::Value(name) => name,
                        BinderSymbol::Type(_) => unreachable!("`zero` is a value name"),
                    }
                ),
                Some(KType::NUMBER),
            );
        })
    });
}

#[test]
fn a_module_captures_an_outer_value_and_holds_a_module_of_its_own() {
    let source = "\
LET greeting = \"hi\"
MODULE outer = ((MODULE inner = (LET n = 1)) (LET f = (FN :{} -> Str = (greeting))))";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        fixture.in_cell(pin, |context, binder| {
            let activation = fixture.run(context.writer(), &lines, binder, &[]);
            let outer = module(fixture, activation, "outer");
            let node = outer.module().expect("a module node");
            let body = activation
                .shape()
                .births(activation.shape().slot(fixture.name("outer")).unwrap().0)
                .expect("a module binder births its body");
            assert_eq!(
                body.captures().len(),
                1,
                "the body's function reads an outer name through the module",
            );
            let member = |name: &str| {
                let (slot, _) = body.slot(fixture.name(name)).expect("a declared member");
                node.members()[slot.index()]
            };
            assert!(member("f").as_callable().is_some());

            // A module bound in another module's body is a value slot at a Signature handle.
            let inner = member("inner").as_module().expect("a nested module");
            assert!(matches!(
                fixture.types.node(inner.ktype()),
                TypeNode::Signature { .. }
            ));
            let TypeNode::Signature { schema, .. } = fixture.types.node(outer.ktype()) else {
                panic!("a module's type is a Signature node");
            };
            assert_eq!(
                crate::type_lattice::member(
                    schema.value_slots,
                    match fixture.name("inner") {
                        BinderSymbol::Value(name) => name,
                        BinderSymbol::Type(_) => unreachable!("`inner` is a value name"),
                    }
                ),
                Some(inner.ktype()),
            );
        })
    });
}

#[test]
fn a_group_binder_births_a_module_the_same_way() {
    let source = "GROUP g FOLD LEFT = (LET step = 1)";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        fixture.in_cell(pin, |context, binder| {
            let activation = fixture.run(context.writer(), &lines, binder, &[]);
            let g = module(fixture, activation, "g");
            assert_eq!(g.module().expect("a module node").members().len(), 1);
        })
    });
}

#[test]
fn a_module_whose_body_ties_a_knot_holds_each_member_callable() {
    let source = "\
MODULE m = ((LET f = (FN :{} -> Number = (g))) (LET g = (FN :{} -> Number = (f))))";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        fixture.in_cell(pin, |context, binder| {
            let activation = fixture.run(context.writer(), &lines, binder, &[]);
            let m = module(fixture, activation, "m");
            let node = m.module().expect("a module node");
            assert_eq!(node.members().len(), 2);
            let members: Vec<Knotted<'_, '_>> = node
                .members()
                .iter()
                .map(|value| value.as_callable().expect("a callable member"))
                .collect();
            assert_eq!(
                members[0].member().knot().len(),
                2,
                "the body's two functions are one knot the module does not own",
            );
            assert!(!ptr::eq(members[0].node(), members[1].node()));
        })
    });
}

#[test]
fn two_modules_are_incomparable_and_render_as_their_signature() {
    let source = "MODULE m = (LET x = 1)\nMODULE n = (LET x = 1)";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context, binder| {
            let activation = fixture.run(context.writer(), &lines, binder, &[]);
            let (m, n) = (
                Value::Knotted(module(fixture, activation, "m")),
                Value::Knotted(module(fixture, activation, "n")),
            );
            assert_eq!(m.equals(&n, types, scratch), Err(Incomparable));
            assert_eq!(m.equals(&m, types, scratch), Err(Incomparable));
            let mut rendered = String::new();
            m.render(&mut rendered, types, fixture.symbols, scratch)
                .unwrap();
            assert_eq!(
                rendered,
                crate::type_lattice::display_name(m.ktype(), types, fixture.symbols).to_string(),
            );
        })
    });
}

#[test]
fn a_module_member_with_no_body_supplied_is_eager_at_its_bodys_site() {
    let source = "MODULE m = (LET x = 1)";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        fixture.in_cell(pin, |context, binder| {
            let activation = fixture.run(context.writer(), &lines, binder, &["m"]);
            let shape = activation.shape();
            let (slot, _) = shape.slot(fixture.name("m")).unwrap();
            let refused = tie(
                context.writer(),
                activation,
                shape.component_of(slot),
                fixture.types,
                fixture.scratch(),
                &mut |_| None,
            )
            .map(|_| ())
            .expect_err("no body was supplied");
            assert_eq!(
                refused,
                Untieable::Eager {
                    name: fixture.name("m"),
                    site: shape.birth_site(slot).expect("the body sits at a site"),
                },
            );
        })
    });
}

#[test]
fn a_claimed_slot_in_the_supplied_body_leaves_the_module_pending() {
    let source = "MODULE m = ((LET x = 1) (LET y = 2))";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &["m"]);
            let shape = activation.shape();
            let (slot, _) = shape.slot(fixture.name("m")).unwrap();
            let body = crate::memory::resident(
                writer,
                body_activation(writer, activation, slot, fixture.scratch())
                    .expect("the body captures nothing"),
            );
            for index in 0..body.shape().slots() {
                body.claim(Slot(index as u32), binder).unwrap();
            }
            let (x, _) = body
                .shape()
                .slot(fixture.name("x"))
                .expect("`x` is declared");
            body.bind(x, Value::Number(1.0)).unwrap();
            let refused = tie(
                writer,
                activation,
                shape.component_of(slot),
                fixture.types,
                fixture.scratch(),
                &mut |_| Some(Supplied::Body(body)),
            )
            .map(|_| ())
            .expect_err("`y` is still claimed");
            assert_eq!(
                refused,
                Untieable::Pending {
                    name: fixture.name("y"),
                    binder,
                },
            );
        })
    });
}

#[test]
fn a_pending_capture_refuses_the_body_activation_and_writes_nothing() {
    let source = "\
LET later = 1
MODULE m = (LET held = later)";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        fixture.in_cell(pin, |context, binder| {
            let activation = fixture.run(context.writer(), &lines, binder, &["later", "m"]);
            let shape = activation.shape();
            let (slot, _) = shape.slot(fixture.name("m")).unwrap();
            assert!(matches!(
                read(fixture, activation, "later"),
                Binding::Pending(_)
            ));
            let refused = body_activation(context.writer(), activation, slot, fixture.scratch())
                .map(|_| ())
                .expect_err("the capture is pending");
            assert_eq!(
                refused,
                Untieable::Pending {
                    name: fixture.name("later"),
                    binder,
                },
            );
        })
    });
}

#[test]
fn a_type_member_is_read_back_through_the_declaration_door() {
    let source = "MODULE m = (NEWTYPE Dist = Number)\nNEWTYPE Outer = Str";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        fixture.in_cell(pin, |context, binder| {
            let activation = fixture.run(context.writer(), &lines, binder, &[]);
            assert_eq!(declared(fixture, activation, "Outer"), {
                let TypeNode::SetMember { .. } =
                    fixture.types.node(declared(fixture, activation, "Outer"))
                else {
                    panic!("a newtype is a set member");
                };
                declared(fixture, activation, "Outer")
            });
            let m = module(fixture, activation, "m");
            let Value::Type(dist) = m.module().expect("a module node").members()[0] else {
                panic!("a type member is a type value");
            };
            assert_ne!(
                dist.handle(),
                KType::NUMBER,
                "a newtype is its own identity"
            );
            let _ = callable;
        })
    });
}
