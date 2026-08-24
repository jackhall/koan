//! Per-component member identity: the invariants ruling 10 names, plus the sibling-edge walk and
//! the condensation ordering the seal rests on.

use super::super::record::Record;
use super::*;
use crate::machine::model::RunRegistries;

/// A fixture member, binder or parameter name as the Type token the window keys by. Pure
/// classification — these tests compare identities and never render one.
fn name(text: &str) -> TypeSymbol {
    TypeSymbol::classify(text).expect("a fixture member name is a Type token")
}

/// A record type over `pairs`.
fn record(types: &TypeRegistry, pairs: Vec<(&str, KType)>) -> KType {
    types.record(Record::from_pairs(
        pairs
            .into_iter()
            .map(|(n, t)| (crate::machine::model::Symbol::of(n), t)),
    ))
}

fn sibling(types: &TypeRegistry, index: usize) -> KType {
    types.intern(TypeNode::Sibling(index))
}

fn newtype(repr: KType) -> RelativeSchema {
    RelativeSchema::NewType(repr)
}

/// Seal a window over `members`, each `(name, schema-builder)`, and return the member handles in
/// declaration order.
fn seal(types: &TypeRegistry, members: Vec<(&str, RelativeSchema)>) -> Vec<KType> {
    let window = RecursiveGroupWindow::new(
        members
            .iter()
            .map(|(text, schema)| (name(text), schema.kind()))
            .collect(),
    );
    let count = members.len();
    let mut sealed = None;
    for (index, (_, schema)) in members.into_iter().enumerate() {
        sealed = window.fill_member(index, schema, types);
    }
    let sealed = sealed.expect("the last fill seals a fully declared window");
    assert_eq!(sealed.members.len(), count);
    sealed.members
}

/// The component digest and size a member handle was derived from.
fn placement(types: &TypeRegistry, member: KType) -> (TypeDigest, usize) {
    match types.node(member) {
        TypeNode::SetMember {
            scc_digest,
            scc_size,
            ..
        } => (scc_digest, scc_size),
        _ => panic!("a sealed member must intern as a SetMember node"),
    }
}

/// A standalone declaration and the same declaration co-declared beside an unrelated member are
/// the same type: the unreferenced member is in nobody's component, so it is in nobody's fold.
#[test]
fn unreferenced_co_declared_member_does_not_move_a_sibling() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let alone = seal(types, vec![("Meters", newtype(KType::NUMBER))]);
    let beside = seal(
        types,
        vec![
            ("Meters", newtype(KType::NUMBER)),
            ("Unrelated", newtype(KType::STR)),
        ],
    );
    assert_eq!(
        alone[0], beside[0],
        "a co-declared member that nobody references must perturb no identity",
    );
}

/// A non-recursive member declared inside a group is a singleton component, so it unifies with the
/// standalone declaration of the same content — the declaration boundary is not identity.
#[test]
fn non_recursive_group_member_equals_its_standalone_twin() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let standalone =
        RecursiveGroupWindow::seal_singleton(name("Leaf"), newtype(KType::NUMBER), None, types);
    // `Leaf` sits in a group beside a self-recursive `Trunk` that does not name it.
    let grouped = seal(
        types,
        vec![
            (
                "Trunk",
                newtype(record(types, vec![("next", sibling(types, 0))])),
            ),
            ("Leaf", newtype(KType::NUMBER)),
        ],
    );
    assert_eq!(
        standalone, grouped[1],
        "a non-recursive member is a singleton component and unifies with its standalone twin",
    );
}

/// Declaration order is presentation. Two groups declaring one mutually-recursive pair in opposite
/// orders produce the same component and the same per-name identities.
#[test]
fn member_declaration_order_is_immaterial() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    // Odd references Even, Even references Odd.
    let odd_first = seal(
        types,
        vec![
            (
                "Odd",
                newtype(record(types, vec![("pred", sibling(types, 1))])),
            ),
            (
                "Even",
                newtype(record(types, vec![("pred", sibling(types, 0))])),
            ),
        ],
    );
    let even_first = seal(
        types,
        vec![
            (
                "Even",
                newtype(record(types, vec![("pred", sibling(types, 1))])),
            ),
            (
                "Odd",
                newtype(record(types, vec![("pred", sibling(types, 0))])),
            ),
        ],
    );
    assert_eq!(odd_first[0], even_first[1], "Odd is Odd either way round");
    assert_eq!(odd_first[1], even_first[0], "Even is Even either way round");
    let (digest, size) = placement(types, odd_first[0]);
    assert_eq!(size, 2, "the pair is one two-member component");
    assert_eq!(
        digest,
        placement(types, odd_first[1]).0,
        "both members name the same component",
    );
}

/// Content is content: two groups alike but for one external reference stay distinct, because a
/// cross-component reference folds the referent's own digest into the component.
#[test]
fn external_reference_distinguishes_otherwise_identical_groups() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let over = |leaf: KType| {
        seal(
            types,
            vec![(
                "Wrapper",
                newtype(record(
                    types,
                    vec![("value", leaf), ("next", sibling(types, 0))],
                )),
            )],
        )[0]
    };
    assert_ne!(
        over(KType::STR),
        over(KType::NUMBER),
        "a differing external reference must not unify",
    );
}

/// A member referencing an upstream component folds that component's *finished* handle, so the two
/// components are separately identified and the downstream one is not part of the upstream's fold.
#[test]
fn cross_component_reference_folds_the_finished_handle() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    // `Forest` names `Tree`; `Tree` is self-recursive and never names `Forest`.
    let members = seal(
        types,
        vec![
            (
                "Forest",
                newtype(record(types, vec![("trees", sibling(types, 1))])),
            ),
            (
                "Tree",
                newtype(record(types, vec![("child", sibling(types, 1))])),
            ),
        ],
    );
    let (forest_digest, forest_size) = placement(types, members[0]);
    let (tree_digest, tree_size) = placement(types, members[1]);
    assert_eq!(forest_size, 1, "Forest is its own component");
    assert_eq!(tree_size, 1, "Tree is its own component");
    assert_ne!(forest_digest, tree_digest);

    // The same `Tree` declared alone is the same type — `Forest` is downstream, so it is not in
    // Tree's fold.
    let alone = RecursiveGroupWindow::seal_singleton(
        name("Tree"),
        newtype(record(types, vec![("child", sibling(types, 0))])),
        None,
        types,
    );
    assert_eq!(
        alone, members[1],
        "a member nothing upstream constrains keeps its standalone identity",
    );
}

/// A sealed member's schema holds absolute handles at every depth, including the cyclic edge back
/// to itself — which is what makes the group's composition edges navigable without a window.
#[test]
fn sealed_schema_is_absolute_and_cyclic() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let chain = RecursiveGroupWindow::seal_singleton(
        name("Chain"),
        newtype(record(
            types,
            vec![("head", KType::NUMBER), ("tail", sibling(types, 0))],
        )),
        None,
        types,
    );
    let schema = match types.node(chain) {
        TypeNode::SetMember { schema, .. } => schema,
        _ => panic!("expected a SetMember node"),
    };
    let repr = match schema {
        NodeSchema::NewType(repr) => repr,
        _ => panic!("expected a NewType schema"),
    };
    let fields = match types.node(repr) {
        TypeNode::Record { fields } => fields,
        _ => panic!("expected a record repr"),
    };
    assert_eq!(
        fields
            .get(crate::machine::model::Symbol::of("tail"))
            .copied(),
        Some(chain),
        "the self-reference seals to the member's own absolute handle",
    );
}

/// A generative window's nonce folds into its member's component, so two mints of identical
/// content stay distinct while a nonce-free mint of the same content is content-addressed.
#[test]
fn generative_nonce_separates_two_mints() {
    use crate::machine::core::ScopeId;
    let registries = RunRegistries::new();
    let types = &registries.types;
    let mint = |nonce: Option<ScopeId>| {
        RecursiveGroupWindow::seal_singleton(name("Opaque"), newtype(KType::NUMBER), nonce, types)
    };
    let first = mint(Some(ScopeId::from_raw(0, 1)));
    let second = mint(Some(ScopeId::from_raw(0, 2)));
    assert_ne!(first, second, "two applications never unify");
    assert_ne!(
        first,
        mint(None),
        "a nonce must change the component digest"
    );
    assert_eq!(mint(None), mint(None), "a nonce-free mint is content-keyed");
}

/// A `TypeConstructor` member's variant schema binds its sibling references the same way a
/// `NewType` representation does.
#[test]
fn type_constructor_schema_binds_siblings() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let schema: TypeMemberMap = [
        (name("Empty"), KType::NULL),
        (name("Full"), sibling(types, 0)),
    ]
    .into_iter()
    .collect();
    let handle = RecursiveGroupWindow::seal_singleton(
        name("Maybe"),
        RelativeSchema::TypeConstructor {
            schema,
            param_names: vec![name("Elem")],
        },
        None,
        types,
    );
    match types.node(handle) {
        TypeNode::SetMember {
            kind,
            schema: NodeSchema::TypeConstructor { schema, .. },
            ..
        } => {
            assert_eq!(kind, KKind::TypeConstructor);
            assert_eq!(schema.get(&name("Full")).copied(), Some(handle));
        }
        _ => panic!("expected a TypeConstructor SetMember"),
    }
}

/// The window announces a member on first reference, so a declarator that discovers its members as
/// it walks its own schema still mints stable relative handles.
#[test]
fn sibling_announces_an_unseen_name() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let window = RecursiveGroupWindow::new(vec![(name("Leaf"), KKind::NewType)]);
    assert_eq!(
        window.sibling(name("Leaf"), KKind::NewType, types),
        sibling(types, 0)
    );
    assert_eq!(
        window.sibling(name("Node"), KKind::NewType, types),
        sibling(types, 1)
    );
    assert_eq!(window.len(), 2);
    assert_eq!(
        window.unfilled_member_names(),
        vec![name("Leaf"), name("Node")]
    );
    assert!(window.member_index(name("Node")).is_some());
    assert!(window.member_index(name("Absent")).is_none());
}

/// The binder name of a `UNION` denotes the union of exactly the variants it owns, not any one of
/// them — and an owned variant answers no bare-name lookup, only the qualified one scoped by its
/// binder.
#[test]
fn binder_union_covers_every_owned_member() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let window = RecursiveGroupWindow::for_binder(name("Maybe"), vec![name("Some"), name("None")]);
    assert!(window.binds(name("Maybe")));
    assert_eq!(
        window.binder_union(name("Maybe"), types),
        Some(types.union_of(vec![sibling(types, 0), sibling(types, 1)])),
    );
    assert_eq!(
        window.variant_index(name("Maybe"), name("None").symbol()),
        Some(1)
    );
    assert_eq!(
        window.member_index(name("Some")),
        None,
        "a variant is owned by its binder, so it answers no bare-name lookup",
    );
}

/// The canonical component order sorts on the bare tag with the owning binder as tiebreak, and the
/// owner never enters the digest — so two same-tag variants under different binders take stable
/// distinct positions in one component, and a module-hosted variant digests like its standalone
/// twin.
#[test]
fn same_tag_under_two_binders_seals_to_distinct_members() {
    use super::{SealBinderInput, SealMemberInput, seal_group};
    let registries = RunRegistries::new();
    let types = &registries.types;
    let members = vec![
        SealMemberInput {
            name: name("Node"),
            owner: Some(name("Graph")),
            kind: KKind::NewType,
            schema: newtype(KType::NUMBER),
        },
        SealMemberInput {
            name: name("Node"),
            owner: Some(name("Tree")),
            kind: KKind::NewType,
            schema: newtype(KType::STR),
        },
    ];
    let binders = vec![
        SealBinderInput {
            name: name("Graph"),
            members: &[0],
        },
        SealBinderInput {
            name: name("Tree"),
            members: &[1],
        },
    ];
    let sealed = seal_group(&members, &binders, None, types);
    assert_ne!(
        sealed.members[0], sealed.members[1],
        "differing payloads digest apart",
    );
    assert_eq!(sealed.binder_types.len(), 2);
    assert_eq!(
        sealed.binder_type(name("Graph")),
        Some(types.union_of(vec![sealed.members[0]])),
    );
    // The owner orders but does not digest: `Graph Node` over Number is byte-identical to a
    // standalone `Node` over Number.
    let standalone = super::RecursiveGroupWindow::seal_singleton(
        name("Node"),
        newtype(KType::NUMBER),
        None,
        types,
    );
    assert_eq!(sealed.members[0], standalone);
}

/// Tarjan emits a component only after every component it references, which is the order the seal
/// consumes: a cross-component reference always has a finished handle by the time it is folded.
#[test]
fn condensation_is_emitted_successor_first() {
    // 0 → 1, 1 ↔ 2, 3 isolated.
    let components = tarjan_components(&[vec![1], vec![2], vec![1], vec![]]);
    let position = |member: usize| {
        components
            .iter()
            .position(|c| c.contains(&member))
            .expect("every member lands in a component")
    };
    assert!(
        position(1) < position(0),
        "the referenced component must be emitted first",
    );
    assert_eq!(position(1), position(2), "1 and 2 are one component");
    assert_eq!(components.len(), 3);
}
