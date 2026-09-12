//! The registry's allocation contract, as a count.
//!
//! No law: whether a door reaches the global heap is a fact about where it builds its buffers, not
//! about the algebra, so it is pinned by a bracket around a battery. The lib-test binary's counting
//! allocator ([`allocation_count`]) tallies this thread's heap allocations. Every region the
//! battery touches is first grown to a chunk it fits in, and the verdict table — the one heap-owned
//! part of the registry — is pre-sized, so any allocation inside the bracket is the lattice's own,
//! and the test names it by failing.

use crate::memory::{BumpAllocator, BumpVec, ScopeId};
use crate::parse::{BinderSymbol, KeywordSymbol, LabelInterner, TypeSymbol, ValueSymbol};
use crate::tests::allocation_count;

use crate::type_lattice::handle::KType;
use crate::type_lattice::kind::KKind;
use crate::type_lattice::lattice::{join, meet};
use crate::type_lattice::node::TypeNode;
use crate::type_lattice::operators::ReductionMode;
use crate::type_lattice::order::is_subtype_of;
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::schema::{SchemaDraft, specialize_schema};
use crate::type_lattice::shape::DispatchTokenElement::{Keyword, Slot};
use crate::type_lattice::sig_relations::{
    SigSubtypeFailure, select_keyworded_satisfier, shape_specificity, sig_subtype,
};
use crate::type_lattice::substitute::{
    canonicalize_binder, erase_quantified, substitute_quantified,
};
use crate::type_lattice::unify::{Collector, UnifyFailure, admits_with};
use crate::type_lattice::walk::Variance;
use crate::type_lattice::window::{RecursiveGroupWindow, RelativeSchema};

use super::generators::{allocator, fresh_cart};

/// Grow `region`'s bump to a chunk the whole battery fits in, then hand the bytes back, so nothing
/// inside the bracket asks the heap for a chunk of its own. A dropped vector is its bump's newest
/// allocation, which the bump takes back whole.
fn warm(region: BumpAllocator<'_>) {
    drop(BumpVec::<u8>::with_capacity_in(1 << 20, region));
}

#[test]
fn interning_and_relations_touch_no_heap() {
    // Every label is declared before the bracket: declaring one interns its text.
    let labels = LabelInterner::new();
    let x = BinderSymbol::declared("x", &labels).expect("a bindable token");
    let y = BinderSymbol::declared("y", &labels).expect("a bindable token");
    let elt = TypeSymbol::declared("Elt", &labels).expect("a Type token");
    let item = TypeSymbol::declared("Item", &labels).expect("a Type token");
    let wrap = TypeSymbol::declared("Wrap", &labels).expect("a Type token");
    let pure = KeywordSymbol::declared("PURE", &labels).expect("a keyword token");
    let plus = KeywordSymbol::declared("PLUS", &labels).expect("a keyword token");
    let a = ValueSymbol::declared("a", &labels).expect("a value token");

    // The registry's region, the relations' scratch, and the declaring frame a window lives in.
    let (registry_cart, scratch_cart, host_cart) = (fresh_cart(), fresh_cart(), fresh_cart());
    let region = allocator(&registry_cart);
    let scratch = allocator(&scratch_cart);
    let host = allocator(&host_cart);
    for bump in [region, scratch, host] {
        warm(bump);
    }
    let types = TypeRegistry::in_region(region);
    types.reserve_verdicts(1 << 16);

    let before = allocation_count();

    // --- Interning ---
    let record = types.record(scratch, &[(x, KType::NUMBER), (y, KType::STR)]);
    let narrow = types.record(scratch, &[(x, KType::NUMBER)]);
    let function = types.function_type(scratch, &[(x, KType::NUMBER)], record);
    let union = types.union_of(scratch, &[KType::NUMBER, KType::STR, record]);
    let variable = types.quantified(0, KType::NUMBER);
    let shape = types
        .shape_type(
            scratch,
            &[elt],
            &[Keyword(pure), Slot(variable), Slot(variable)],
            variable,
        )
        .handle;
    let plain = types
        .shape_type(
            scratch,
            &[],
            &[Keyword(pure), Slot(KType::NUMBER), Slot(KType::NUMBER)],
            KType::NUMBER,
        )
        .handle;

    // A signature with a member of every kind and a chaining record.
    let member = types.abstract_type(scratch, ScopeId::SENTINEL, elt, &[], None, KType::ANY);
    let family = types.abstract_type(scratch, ScopeId::SENTINEL, wrap, &[item], None, KType::ANY);
    let operator = types
        .shape_type(
            scratch,
            &[],
            &[Slot(member), Keyword(plus), Slot(member)],
            member,
        )
        .handle;
    let mut draft = SchemaDraft::new(scratch);
    draft.sig_id = Some(ScopeId::SENTINEL);
    draft.insert_abstract(elt, member);
    draft.insert_abstract(wrap, family);
    draft.insert_manifest(item, KType::NUMBER);
    draft.insert_value_slot(a, member);
    draft.push_keyworded(operator);
    draft.push_operator_group(&[plus], ReductionMode::FoldLeft);
    let interface = types.signature(scratch, draft);
    // A module missing the constructor member the interface declares.
    let mut draft = SchemaDraft::new(scratch);
    draft.insert_manifest(elt, KType::NUMBER);
    let module = types.signature(scratch, draft);
    // A keyworded interface, and a module whose only overload under that key misses it.
    let wants = types
        .shape_type(
            scratch,
            &[],
            &[Keyword(pure), Slot(KType::NUMBER)],
            KType::NUMBER,
        )
        .handle;
    let offers = types
        .shape_type(
            scratch,
            &[],
            &[Keyword(pure), Slot(KType::STR)],
            KType::NUMBER,
        )
        .handle;
    let mut draft = SchemaDraft::new(scratch);
    draft.push_keyworded(wants);
    let keyworded = types.signature(scratch, draft);
    let mut draft = SchemaDraft::new(scratch);
    draft.push_keyworded(offers);
    let mismatched = types.signature(scratch, draft);
    // A module fixing the member `module` fixes to a different type: the two have no meet.
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
    let constructor = RelativeSchema::constructor(host, scratch, &[(elt, back)], &[item]);
    let sealed = window
        .fill_member(2, constructor, &types, scratch)
        .expect("the last fill seals");
    let group_member = sealed.member(0).expect("every member seals");

    let TypeNode::Signature { schema, .. } = types.node(interface) else {
        unreachable!("the signature door interns a signature");
    };
    let specialized = specialize_schema(&types, scratch, schema, &[(elt, KType::STR)]);

    // --- Relations ---
    assert!(is_subtype_of(&types, scratch, record, narrow));
    assert!(!is_subtype_of(&types, scratch, narrow, record));
    let _ = is_subtype_of(&types, scratch, specialized, interface);
    let _ = join(&types, scratch, record, function);
    let _ = join(&types, scratch, group_member, KType::NUMBER);
    let _ = meet(&types, scratch, record, narrow);
    let _ = meet(&types, scratch, union, record);
    // Two signatures meet member for member; two manifest bindings for one name have no meet.
    assert_ne!(meet(&types, scratch, interface, module), KType::NEVER);
    assert_eq!(meet(&types, scratch, module, clashing), KType::NEVER);

    let mut collector = Collector::new(scratch, 1);
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
    let mut split = Collector::new(scratch, 1);
    for argument in [KType::NUMBER, KType::STR] {
        assert!(
            admits_with(
                &types,
                scratch,
                variable,
                argument,
                Variance::Co,
                &mut split
            )
            .is_ok()
        );
    }
    assert!(matches!(
        split.solve(&types),
        Err(UnifyFailure::NoMaximum { index: 0, .. })
    ));

    let read = |kt: KType| match types.node(kt) {
        TypeNode::Signature { schema, .. } => schema,
        _ => unreachable!("the signature door interns a signature"),
    };
    assert!(sig_subtype(&types, scratch, schema, schema).is_ok());
    assert!(sig_subtype(&types, scratch, read(module), schema).is_err());
    assert!(matches!(
        sig_subtype(&types, scratch, read(mismatched), read(keyworded)),
        Err(SigSubtypeFailure::KeywordedMismatch { .. })
    ));
    // Two unordered signatures: the union door's subsumption pass takes both negative verdicts.
    let _ = types.union_of(scratch, &[interface, module]);
    let _ = shape_specificity(&types, scratch, shape, plain);
    assert!(select_keyworded_satisfier(&types, scratch, wants, &[wants, offers], None).is_ok());
    let _ = substitute_quantified(&types, scratch, types.list(variable), &[KType::STR]);
    let _ = erase_quantified(&types, scratch, shape);
    let _ = canonicalize_binder(&types, scratch, interface, ScopeId::SENTINEL);
    assert!(types.contains_quantified(types.list(variable)));
    assert!(types.contains_rigid(family));

    let allocated = allocation_count() - before;
    assert_eq!(
        allocated, 0,
        "interning and the relations made {allocated} heap allocations"
    );
}
