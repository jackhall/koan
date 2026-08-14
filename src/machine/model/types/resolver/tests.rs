use super::*;
use crate::builtins::test_support::{TestRun, mock_declaration_site};
use crate::machine::core::{FrameStorageExt, program_storage, run_root_storage};
use crate::machine::model::Record;
use crate::machine::model::Scalar;
use crate::machine::model::ast::TypeIdentifier;
use crate::machine::model::values::Carried;
use crate::machine::model::{AnnouncedData, RelativeSchema};
use crate::machine::{BindingIndex, DeclarationSite};

fn leaf(n: &str) -> TypeIdentifier<'_> {
    TypeIdentifier::leaf(n)
}

/// A module-body child announcing `members` as standalone type declarations — the ambient window a
/// body statement elaborates against.
fn announced_module<'a>(
    parent: &'a crate::machine::Scope<'a>,
    members: &[&str],
) -> &'a crate::machine::Scope<'a> {
    let mut announced = AnnouncedData::default();
    for member in members {
        announced.announce(member.to_string());
    }
    parent.alloc_child_under_module("m", Some(announced))
}

/// A Type token cannot name a value — the binding maps enforce the token-class partition — so a
/// Type-class leaf that names no type is an ordinary unknown-name miss, with no value side to
/// consult. The bind that would set up the old "value-language only" layering is itself rejected.
#[test]
fn type_token_cannot_bind_value_side() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let error = scope
        .bind_value_direct(
            "Gee".into(),
            scope.seal_resident(Carried::Object(
                region.brand().alloc_scalar(Scalar::Number(7.0)),
            )),
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect_err("a Type token names a type; it may not bind a value");
    assert!(
        matches!(&error.kind, crate::machine::KErrorKind::ShapeError(msg)
            if msg.contains("`Gee` is a Type token")),
        "expected the token-class partition error, got {error}",
    );
    let types = test_run.types.clone();
    let mut el = Elaborator::new(scope);
    match elaborate_type_identifier(&mut el, &leaf("Gee"), &types) {
        TypeResolution::Unbound(msg) => assert!(
            msg.contains("Gee"),
            "expected an unknown-name miss naming `Gee`, got: {msg}",
        ),
        other => panic!("expected Unbound, got {:?}", other),
    }
}

#[test]
fn unbound_leaf_names_unknown_type() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = test_run.types.clone();
    let mut el = Elaborator::new(scope);
    match elaborate_type_identifier(&mut el, &leaf("NopeType"), &types) {
        TypeResolution::Unbound(msg) => assert!(
            msg.contains("unknown type name") && msg.contains("NopeType"),
            "expected an unknown-type-name message naming `NopeType`, got: {msg}",
        ),
        other => panic!("expected Unbound, got {:?}", other),
    }
}

/// A bare leaf naming an announced member lowers, **for the declarator**, to that member's relative
/// sibling handle — the module body's cross-order resolution, independent of source order. A
/// non-member falls through to ordinary resolution.
#[test]
fn announced_member_lowers_to_sibling_for_a_declarator() {
    let program = program_storage();
    let region = run_root_storage();
    let parent_test_run = TestRun::silent(&program, &region);
    let parent = parent_test_run.scope;
    let child = announced_module(parent, &["A", "B"]);
    let window = child.own_declaration_window().expect("the body announced");
    let types = parent_test_run.types.clone();
    let mut el = Elaborator::new(child).with_window(WindowView::Announced(window));
    match elaborate_type_identifier(&mut el, &leaf("B"), &types) {
        TypeResolution::Done(kt) => assert_eq!(kt, types.intern(TypeNode::Sibling(1))),
        other => panic!("expected a sibling back-edge for a window member, got {other:?}"),
    }
    let mut el2 = Elaborator::new(child).with_window(WindowView::Announced(window));
    assert!(
        matches!(
            elaborate_type_identifier(&mut el2, &leaf("Nope"), &types),
            TypeResolution::Unbound(_)
        ),
        "a non-member must fall through to ordinary resolution",
    );
}

/// The same reference from a **consumer** never takes the relative handle: with no producer to park
/// on it is a typed miss, and never a `Sibling` a dispatch could silently fail to match.
#[test]
fn announced_member_never_lowers_to_sibling_for_a_consumer() {
    let program = program_storage();
    let region = run_root_storage();
    let parent_test_run = TestRun::silent(&program, &region);
    let child = announced_module(parent_test_run.scope, &["A", "B"]);
    let types = parent_test_run.types.clone();
    let mut el = Elaborator::new(child);
    match elaborate_type_identifier(&mut el, &leaf("B"), &types) {
        TypeResolution::Unbound(msg) => assert!(
            msg.contains("co-declared"),
            "expected the dead-declaration miss, got {msg}",
        ),
        other => panic!("a consumer must never observe a pre-seal member, got {other:?}"),
    }
}

/// A `UNION`'s own binder names no single variant: it resolves to the union of every announced
/// member, which is what a variant payload referencing the union's own name means.
#[test]
fn window_binder_resolves_to_the_union_of_its_members() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let types = test_run.types.clone();
    let window =
        RecursiveGroupWindow::for_binder("Tree".into(), vec!["Leaf".into(), "Node".into()]);
    let mut el = Elaborator::new(test_run.scope).with_window(WindowView::Local(&window));
    match elaborate_type_identifier(&mut el, &leaf("Tree"), &types) {
        TypeResolution::Done(kt) => {
            assert_eq!(Some(kt), window.binder_union("Tree", &types))
        }
        other => panic!("expected the binder union, got {other:?}"),
    }
}

/// A member of a multi-member window has no identity until the whole window seals: identity is
/// computed over the group's entire reference structure, so an intermediate fill defers. The last
/// fill seals, and only then does the member's handle install.
#[test]
fn announced_member_defers_until_the_window_seals() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = announced_module(test_run.scope, &["Node", "Leaf"]);
    let types = test_run.types.clone();
    let window = DeclWindow::Ambient(scope.own_declaration_window().expect("announced"));
    let fill = |name: &str, repr: KType, site: DeclarationSite| {
        finalize_nominal_member(&window, name, |_| repr, site, scope.brand(), &types)
    };
    match fill("Node", KType::NUMBER, mock_declaration_site(2)) {
        SealOutcome::Deferred => {}
        other => panic!(
            "the first of two members must defer, got {}",
            outcome_tag(&other)
        ),
    }
    let (sealed, writes) = match fill("Leaf", KType::STR, mock_declaration_site(3)) {
        SealOutcome::Sealed { kt, writes } => (kt, writes),
        other => panic!("the last fill must seal, got {}", outcome_tag(&other)),
    };
    assert_eq!(
        sealed,
        window
            .view()
            .sealed_member(1)
            .expect("the group sealed at the last fill"),
        "the outcome is Leaf's own member handle",
    );
    // The seal writes nothing itself — the ops it hands back are what install the identities, all
    // of them at once, and the run loop applies them after the declaring step returns.
    assert_eq!(
        writes.len(),
        2,
        "the sealing statement installs every member"
    );
    assert!(scope.bindings().types().get("Leaf").is_none());
    for write in writes {
        write
            .apply(scope, &mut crate::machine::WriteGate::for_test())
            .expect("the first install lands");
    }

    // A different statement declaring `Leaf` over different content is a redeclaration: its op
    // collides with the identity this window installed, so the apply — not the seal — errors.
    let other_window = DeclWindow::Owned(RecursiveGroupWindow::new(vec![(
        "Leaf".into(),
        KKind::NewType,
    )]));
    let redeclare = match finalize_nominal_member(
        &other_window,
        "Leaf",
        |_| KType::BOOL,
        mock_declaration_site(4),
        scope.brand(),
        &types,
    ) {
        SealOutcome::Sealed { writes, .. } => writes.into_iter().next().expect("one write"),
        other => panic!("the singleton window seals, got {}", outcome_tag(&other)),
    };
    let error = redeclare
        .apply(scope, &mut crate::machine::WriteGate::for_test())
        .expect_err("a redeclaration of Leaf must Rebind at apply");
    assert!(
        matches!(&error.kind, crate::machine::KErrorKind::Rebind { name } if name == "Leaf"),
        "expected Rebind naming Leaf, got {error}",
    );
}

fn outcome_tag(outcome: &SealOutcome) -> &'static str {
    match outcome {
        SealOutcome::Sealed { .. } => "Sealed",
        SealOutcome::Deferred => "Deferred",
        SealOutcome::DanglingRef(_) => "DanglingRef",
    }
}

#[test]
fn constructor_apply_name_renders_surface_form() {
    let types = TypeRegistry::new();
    let ctor = RecursiveGroupWindow::seal_singleton(
        "Wrap".into(),
        RelativeSchema::TypeConstructor {
            schema: std::collections::HashMap::new(),
            param_names: vec!["Type".into()],
        },
        None,
        &types,
    );
    let app = types.constructor_apply(
        ctor,
        Record::from_pairs([("Type".to_string(), KType::NUMBER)]),
    );
    assert_eq!(app.name(&types), ":(Wrap {Type = Number})");
}
