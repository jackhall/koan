//! The registry's allocation contract, as a count.
//!
//! No law: whether a door reaches the global heap is a fact about where it builds its buffers, not
//! about the algebra, so it is pinned by a bracket around a battery. The lib-test binary's counting
//! allocator ([`allocation_count`]) tallies this thread's heap allocations. Every region the
//! battery touches is first grown to a chunk it fits in — the verdict table is laid in the registry's
//! region, too — so any allocation inside the bracket is the lattice's own, and the test names it by
//! failing.

use crate::memory::{Bump, BumpAllocator, BumpVec};
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner, TypeSymbol, ValueSymbol};
use crate::tests::allocation_count;

use crate::type_lattice::handle::KType;
use crate::type_lattice::kind::KKind;
use crate::type_lattice::lattice::{join, meet};
use crate::type_lattice::operators::ReductionMode;
use crate::type_lattice::order::is_subtype_of;
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::schema::{SchemaDraft, SigOrigin};
use crate::type_lattice::shape::DispatchTokenElement::{Keyword, Slot};
use crate::type_lattice::sig_relations::{FitsFailure, shape_specificity, sig_fits};
use crate::type_lattice::substitute::{erase_quantified, substitute_quantified};
use crate::type_lattice::unify::{Collector, admits_with};
use crate::type_lattice::walk::Variance;
use crate::type_lattice::window::{RecursiveGroupWindow, RelativeSchema};

/// Grow `region`'s bump to a chunk the whole battery fits in, then hand the bytes back, so nothing
/// inside the bracket asks the heap for a chunk of its own. A dropped vector is its bump's newest
/// allocation, which the bump takes back whole.
fn warm(region: BumpAllocator<'_>) {
    drop(BumpVec::<u8>::with_capacity_in(1 << 20, region));
}

#[test]
fn interning_and_relations_touch_no_heap() {
    // Every symbol is declared before the bracket: declaring one interns its text.
    let symbols = SymbolInterner::new();
    let x = BinderSymbol::declared("x", &symbols).expect("a bindable token");
    let y = BinderSymbol::declared("y", &symbols).expect("a bindable token");
    let elt = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let item = TypeSymbol::declared("Item", &symbols).expect("a Type token");
    let wrap = TypeSymbol::declared("Wrap", &symbols).expect("a Type token");
    let pure = KeywordSymbol::declared("PURE", &symbols).expect("a keyword token");
    let plus = KeywordSymbol::declared("PLUS", &symbols).expect("a keyword token");
    let a = ValueSymbol::declared("a", &symbols).expect("a value token");

    // The registry's region, the relations' scratch, and the declaring frame a window lives in.
    let (registry_bump, scratch_bump, host_bump) = (Bump::new(), Bump::new(), Bump::new());
    let region = &registry_bump;
    let scratch = &scratch_bump;
    let host = &host_bump;
    for bump in [region, scratch, host] {
        warm(bump);
    }
    let types = TypeRegistry::in_region(region);
    // What the two pair solves read: an unbounded variable, the union its lower contributions
    // join to, and a function position it reaches from above with two unrelated arguments.
    let open = types.quantified(0, KType::ANY);
    let mixed = types.union_of(scratch, &[KType::NUMBER, KType::STR]);
    let above = types
        .function_type(scratch, &[], &[], &[(x, open)], KType::NULL)
        .handle;
    let takes = |t: KType| {
        types
            .function_type(scratch, &[], &[], &[(x, t)], KType::NULL)
            .handle
    };
    let (takes_number, takes_str) = (takes(KType::NUMBER), takes(KType::STR));

    let before = allocation_count();

    // --- Interning ---
    let record = types.record(scratch, &[(x, KType::NUMBER), (y, KType::STR)]);
    let narrow = types.record(scratch, &[(x, KType::NUMBER)]);
    let function = types
        .function_type(scratch, &[], &[], &[(x, KType::NUMBER)], record)
        .handle;
    let union = types.union_of(scratch, &[KType::NUMBER, KType::STR, record]);
    let variable = types.quantified(0, KType::NUMBER);
    let quantified_function = types
        .function_type(
            scratch,
            &[elt],
            &[KType::NUMBER],
            &[(x, variable)],
            variable,
        )
        .handle;
    let shape = types
        .shape_type(
            scratch,
            &[elt],
            &[KType::NUMBER],
            &[Keyword(pure), Slot(variable), Slot(variable)],
            &[],
            variable,
        )
        .handle;
    let plain = types
        .shape_type(
            scratch,
            &[],
            &[],
            &[Keyword(pure), Slot(KType::NUMBER), Slot(KType::NUMBER)],
            &[],
            KType::NUMBER,
        )
        .handle;

    // A signature with a head parameter, a member of every kind and a chaining record.
    let member = types.parameter(elt, KType::ANY, None);
    let operator = types
        .shape_type(
            scratch,
            &[],
            &[],
            &[Slot(member), Keyword(plus), Slot(member)],
            &[],
            member,
        )
        .handle;
    let mut draft = SchemaDraft::new(scratch);
    draft.origin = SigOrigin::Declared;
    draft.insert_parameter(elt, member);
    draft.insert_manifest(item, KType::NUMBER);
    draft.insert_value_slot(a, member);
    draft.push_keyworded(operator);
    draft.push_operator_group(&[plus], ReductionMode::FoldLeft);
    let interface = types.signature(scratch, draft);
    // A module missing the members the interface declares.
    let mut draft = SchemaDraft::new(scratch);
    draft.insert_manifest(elt, KType::NUMBER);
    let module = types.signature(scratch, draft);
    // A keyworded interface, and a module whose only overload under that key misses it.
    let wants = types
        .shape_type(
            scratch,
            &[],
            &[],
            &[Keyword(pure), Slot(KType::NUMBER)],
            &[],
            KType::NUMBER,
        )
        .handle;
    let offers = types
        .shape_type(
            scratch,
            &[],
            &[],
            &[Keyword(pure), Slot(KType::STR)],
            &[],
            KType::NUMBER,
        )
        .handle;
    let mut draft = SchemaDraft::new(scratch);
    draft.origin = SigOrigin::Declared;
    draft.push_keyworded(wants);
    let keyworded = types.signature(scratch, draft);
    let mut draft = SchemaDraft::new(scratch);
    draft.push_keyworded(offers);
    let mismatched = types.signature(scratch, draft);
    // A module fixing the member `module` fixes to a different type.
    let mut draft = SchemaDraft::new(scratch);
    draft.insert_manifest(elt, KType::STR);
    let clashing = types.signature(scratch, draft);

    // A three-member recursive group — two newtypes and a constructor — sealed through a window.
    let names = [elt, item, wrap];
    let window = RecursiveGroupWindow::new(
        host,
        &[
            (elt, KKind::NewType),
            (item, KKind::NewType),
            (wrap, KKind::TypeConstructor),
        ],
    );
    for index in 0..2 {
        let next = window.sibling(names[index + 1], KKind::NewType, &types);
        let body = types.union_of(scratch, &[KType::NUMBER, next]);
        assert!(
            window
                .fill_member(index, RelativeSchema::NewType(body), &types, scratch)
                .is_none()
        );
    }
    let back = window.sibling(elt, KKind::NewType, &types);
    let constructor = RelativeSchema::constructor(host, scratch, Some(back), &[item]);
    let sealed = window
        .fill_member(2, constructor, &types, scratch)
        .expect("the last fill seals");
    let group_member = sealed.member(0).expect("every member seals");
    let group_family = sealed.member(2).expect("every member seals");
    let applied = types.constructor_apply(
        scratch,
        group_family,
        &[(BinderSymbol::Type(item), KType::NUMBER)],
    );

    let pinned =
        types.signature_apply(scratch, interface, &[(BinderSymbol::Type(elt), KType::STR)]);

    // --- Relations ---
    assert!(is_subtype_of(&types, scratch, record, narrow));
    assert!(!is_subtype_of(&types, scratch, narrow, record));
    assert!(is_subtype_of(&types, scratch, pinned, interface));
    let _ = join(&types, scratch, record, function);
    let _ = is_subtype_of(&types, scratch, quantified_function, function);
    let _ = join(&types, scratch, group_member, KType::NUMBER);
    let _ = meet(&types, scratch, record, narrow);
    assert!(is_subtype_of(&types, scratch, applied, group_family));
    let sink = types
        .function_type(scratch, &[], &[], &[(x, variable)], KType::NUMBER)
        .handle;
    assert!(types.quantifies_contravariantly(scratch, sink, 1));
    let _ = meet(&types, scratch, union, record);
    // Two signature types meet at the set of both, which is never `Never`.
    assert_ne!(meet(&types, scratch, interface, module), KType::NEVER);
    assert_ne!(meet(&types, scratch, module, clashing), KType::NEVER);

    let mut collector = Collector::new(scratch, &[KType::ANY]);
    assert!(
        admits_with(
            &types,
            scratch,
            variable,
            KType::NUMBER,
            Variance::Co,
            &mut collector
        )
        .is_ok()
    );
    assert!(collector.solve(&types).is_ok());
    let mut least = Collector::least(scratch, 2);
    assert!(
        admits_with(
            &types,
            scratch,
            variable,
            KType::NUMBER,
            Variance::Co,
            &mut least
        )
        .is_ok()
    );
    assert!(least.solve(&types).is_ok());
    let mut split = Collector::new(scratch, &[KType::ANY]);
    for argument in [KType::NUMBER, KType::STR] {
        assert!(admits_with(&types, scratch, open, argument, Variance::Co, &mut split).is_ok());
    }
    assert!(split.solve(&types).is_ok_and(|s| s.as_slice() == [mixed]));
    let mut from_above = Collector::new(scratch, &[KType::ANY]);
    for argument in [takes_number, takes_str] {
        assert!(
            admits_with(
                &types,
                scratch,
                above,
                argument,
                Variance::Co,
                &mut from_above
            )
            .is_ok()
        );
    }
    assert!(from_above.solve(&types).is_ok());

    assert!(sig_fits(&types, scratch, interface, interface).is_ok());
    assert!(sig_fits(&types, scratch, module, interface).is_err());
    assert!(matches!(
        sig_fits(&types, scratch, mismatched, keyworded),
        Err(FitsFailure::KeywordedMismatch { .. })
    ));
    // Two unordered signatures: the union door's subsumption pass takes both negative verdicts.
    let _ = types.union_of(scratch, &[interface, module]);
    let _ = shape_specificity(&types, scratch, shape, plain);
    let _ = substitute_quantified(&types, scratch, types.list(variable), &[KType::STR]);
    let _ = erase_quantified(&types, scratch, shape);
    assert!(types.contains_quantified(types.list(variable)));
    assert!(types.contains_rigid(member));

    let allocated = allocation_count() - before;
    assert_eq!(
        allocated, 0,
        "interning and the relations made {allocated} heap allocations"
    );
}
