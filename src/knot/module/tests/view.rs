//! The view door: what `:!` and `:|` build over a module, and what refuses one.

use crate::knot::tests::{declared, pin, with_fixture};
use crate::memory::ScopeId;
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, TypeNode, sig_subtype};
use crate::values::Value;

use super::super::view::{Ascription, Unascribable, ascribe};
use super::{member, module, schema};

/// `Ord` names two of `m`'s four members. `m` binds `Carrier` itself — a signature's abstract
/// member is satisfied by a *declaration* in the module, not by a type the checker infers.
const PROGRAM: &str = "\
SIG Ord = ((TYPE Carrier) (VAL zero :Carrier))
MODULE m = ((LET Carrier = Number) (LET zero = 0) (LET name = \"m\") (NEWTYPE Dist = Number))";

#[test]
fn a_transparent_view_carries_what_the_signature_names_and_drops_the_rest() {
    with_fixture(|fixture| {
        let lines = fixture.parse(PROGRAM);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let m = module(fixture, activation, "m");
            // `Ord` declares `Carrier` abstract and `zero :Carrier`; `m` binds `zero` to a number,
            // so the source's binding for `Carrier` is `Number`.
            let ord = declared(fixture, activation, "Ord");
            let view = ascribe(writer, m, ord, Ascription::Transparent, types, scratch)
                .unwrap_or_else(|error| panic!("`m` satisfies `Ord`: {error:?}"));

            let node = view.module().expect("a view is a module");
            assert_eq!(node.members().len(), 2, "one value slot, one type member");
            assert!(
                matches!(
                    member(fixture, view, "zero", types, scratch),
                    Value::Number(zero) if zero == 0.0
                ),
                "a transparent view carries the source's own word"
            );
            let Value::Type(carrier) = member(fixture, view, "Carrier", types, scratch) else {
                panic!("`Carrier` is a type member");
            };
            assert_eq!(carrier.handle(), KType::NUMBER);

            // A member the signature does not name is not carried: a view is a narrowing.
            let view_schema = schema(node.ktype(), types);
            assert_eq!(view_schema.value_slots.len(), 1);
            assert!(
                view_schema.abstract_members.is_empty(),
                "a module's signature is concrete"
            );
            assert!(
                sig_subtype(types, scratch, view_schema, schema(ord, types)).is_ok(),
                "the view still satisfies what it was ascribed"
            );
        })
    });
}

#[test]
fn an_opaque_view_mints_a_fresh_carrier_per_application() {
    with_fixture(|fixture| {
        let lines = fixture.parse(PROGRAM);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let m = module(fixture, activation, "m");
            let ord = declared(fixture, activation, "Ord");
            let opaque = |()| {
                ascribe(writer, m, ord, Ascription::Opaque, types, scratch)
                    .unwrap_or_else(|error| panic!("`m` satisfies `Ord`: {error:?}"))
            };
            let (first, second) = (opaque(()), opaque(()));

            let Value::Type(carrier) = member(fixture, first, "Carrier", types, scratch) else {
                panic!("`Carrier` is a type member");
            };
            let mint = carrier.handle();
            assert!(
                matches!(
                    types.node(mint),
                    TypeNode::AbstractType { nonce: Some(_), .. }
                ),
                "an opaque view's carrier is a mint"
            );
            assert_ne!(mint, KType::NUMBER);

            let Value::Type(other) = member(fixture, second, "Carrier", types, scratch) else {
                panic!("`Carrier` is a type member");
            };
            assert_ne!(
                mint,
                other.handle(),
                "two applications of one signature do not unify"
            );
            assert_ne!(
                first.module().expect("a module").ktype(),
                second.module().expect("a module").ktype(),
            );

            // The value member is sealed at the mint, so it no longer reads as a number.
            let zero = member(fixture, first, "zero", types, scratch);
            assert_eq!(zero.ktype(), mint);
            assert!(
                matches!(zero, Value::Tagged(_)),
                "the member is behind the mint, not a bare number"
            );
        })
    });
}

#[test]
fn a_mint_is_sourced_at_its_own_nonce() {
    with_fixture(|fixture| {
        let lines = fixture.parse(PROGRAM);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let m = module(fixture, activation, "m");
            let ord = declared(fixture, activation, "Ord");
            let view = ascribe(writer, m, ord, Ascription::Opaque, types, scratch)
                .unwrap_or_else(|error| panic!("`m` satisfies `Ord`: {error:?}"));
            let Value::Type(carrier) = member(fixture, view, "Carrier", types, scratch) else {
                panic!("`Carrier` is a type member");
            };
            let TypeNode::AbstractType {
                bound,
                nonce,
                source,
                param_names,
                name,
            } = types.node(carrier.handle())
            else {
                panic!("an opaque view's carrier is a mint");
            };
            assert_eq!(
                nonce,
                Some(source),
                "a mint is sourced at the nonce that makes it generative"
            );
            assert_ne!(
                nonce,
                Some(ScopeId::SENTINEL),
                "and never at the canonical binder"
            );
            assert_eq!(BinderSymbol::Type(name), fixture.name("Carrier"));
            assert!(
                param_names.is_empty(),
                "`TYPE Carrier` declares a proper type"
            );
            assert_eq!(
                bound,
                KType::ANY,
                "`TYPE` declares no bound, so the mint stands over Any"
            );
        })
    });
}

#[test]
fn a_view_refuses_what_it_cannot_be() {
    let source = "\
SIG Ord = ((TYPE Carrier) (VAL zero :Carrier))
SIG Wider = ((TYPE Carrier) (VAL zero :Carrier) (VAL one :Carrier))
MODULE m = (LET zero = 0)
LET f = (FN :{} -> Number = (1))";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let m = module(fixture, activation, "m");
            let ord = declared(fixture, activation, "Ord");
            let wider = declared(fixture, activation, "Wider");
            let f = crate::knot::tests::callable(fixture, activation, "f");

            assert!(matches!(
                ascribe(writer, f, ord, Ascription::Opaque, types, scratch),
                Err(Unascribable::NotAModule),
            ));
            assert!(matches!(
                ascribe(writer, m, KType::NUMBER, Ascription::Opaque, types, scratch),
                Err(Unascribable::NotASignature(_)),
            ));
            assert!(matches!(
                ascribe(writer, m, wider, Ascription::Opaque, types, scratch),
                Err(Unascribable::Unsatisfied(_)),
            ));
        })
    });
}

#[test]
fn a_view_copies_across_a_cell_like_any_module() {
    use crate::knot::KValueFamily;
    use crate::knot::tests::{Step, copy};
    use crate::memory::{CellGraph, ReleaseAbsorption};
    use crate::values::cross;

    with_fixture(|fixture| {
        let lines = fixture.parse(PROGRAM);
        let (types, scratch) = (fixture.types, fixture.scratch());
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        let (dormant, mint) = graph
            .enter(home, |context| {
                let writer = context.writer();
                let activation = fixture.run(writer, &lines, &[]);
                let m = module(fixture, activation, "m");
                let ord = declared(fixture, activation, "Ord");
                let view = ascribe(writer, m, ord, Ascription::Opaque, types, scratch)
                    .unwrap_or_else(|error| panic!("`m` satisfies `Ord`: {error:?}"));
                let Value::Type(carrier) = member(fixture, view, "Carrier", types, scratch) else {
                    panic!("`Carrier` is a type member");
                };
                let source = context.lift::<KValueFamily>(Value::Knotted(view));
                let crossed = cross(context, dest, &source).unwrap();
                (context.keep(crossed), carrier.handle())
            })
            .unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        graph
            .enter(dest, |context| {
                let carrier = context.redeem(dormant).unwrap();
                let Value::Knotted(view) = context.read(&carrier).value() else {
                    panic!("the kept view redeems as a module");
                };
                let node = view.module().expect("a view is a module");
                assert_eq!(node.members().len(), 2);
                // The mint is a lattice handle, not a region word, so it rides over verbatim and
                // the sealed member still reads at it.
                let Value::Tagged(zero) = node.members()[0] else {
                    panic!("the sealed member crosses as a tagged value");
                };
                assert_eq!(zero.ktype(), mint);
                assert!(matches!(zero.payload(), Value::Number(n) if *n == 0.0));
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}
