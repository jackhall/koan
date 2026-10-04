//! The builtin-shape door: what an entry's overloads intern to, and that each one still erases to
//! the bucket key a node probes with.

use crate::memory::Bump;
use crate::parse::KeyElement;
use crate::parse::{BUILTIN_SHAPES, BuiltinShapeId, builtin_shape_for};
use crate::type_lattice::{DispatchTokenElement, KType, TypeNode, TypeRegistry};

use super::super::builtin_shape_types;

/// The bucket key an interned handle's elements spell.
fn erasure(types: &TypeRegistry<'_>, handle: KType) -> Vec<KeyElement> {
    let TypeNode::ExpressionShape { elements, .. } = types.node(handle) else {
        panic!("the door interns an expression shape");
    };
    elements
        .iter()
        .map(|element| match element {
            DispatchTokenElement::Keyword(symbol) => KeyElement::Keyword(symbol),
            DispatchTokenElement::Slot(_) => KeyElement::Slot,
        })
        .collect()
}

/// The entry a bucket key probes to, by identity.
fn entry_of(key: &[KeyElement]) -> &'static crate::parse::BuiltinShape {
    builtin_shape_for(key.iter().copied()).expect("an interned overload names a builtin bucket")
}

/// Every overload the door interns erases to its own entry's key — the same key a node's probe
/// carries — so the typed shape and the untyped bucket cannot drift apart.
#[test]
fn every_builtin_shape_erases_to_its_key() {
    let arena = Bump::new();
    let bump = &arena;
    let types = &TypeRegistry::in_region(bump);

    for shape in BUILTIN_SHAPES {
        for handle in builtin_shape_types(shape, types, bump) {
            let key = erasure(types, *handle);
            assert!(
                shape.matches(key.iter().copied()),
                "{:?} interns an overload that erases elsewhere",
                shape.id
            );
            assert!(
                std::ptr::eq(entry_of(&key), shape),
                "{:?}'s overload probes to another entry",
                shape.id
            );
        }
    }
}

/// One handle per overload, and none for a reserved bucket: the door is what says how many typed
/// shapes a builtin key stands for.
#[test]
fn a_bucket_interns_one_handle_per_overload() {
    let arena = Bump::new();
    let bump = &arena;
    let types = &TypeRegistry::in_region(bump);

    for shape in BUILTIN_SHAPES {
        let handles = builtin_shape_types(shape, types, bump);
        let expected = if shape.reserved { 0 } else { shape.overloads() };
        assert_eq!(
            handles.len(),
            expected,
            "{:?} interns the wrong number of overloads",
            shape.id
        );
        let mut distinct = handles.to_vec();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            handles.len(),
            "{:?} interns two overloads to one handle",
            shape.id
        );
    }

    let count =
        |id: BuiltinShapeId| builtin_shape_types(&BUILTIN_SHAPES[id as usize], types, bump).len();
    assert_eq!(count(BuiltinShapeId::NewTypeDefinition), 1);
    assert_eq!(count(BuiltinShapeId::Attribute), 3);
    assert_eq!(count(BuiltinShapeId::CombinedLambda), 0);
    assert_eq!(count(BuiltinShapeId::CombinedQuantifiedLambda), 0);
}
