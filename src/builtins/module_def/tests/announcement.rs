//! A module body announces its top-level type declarations before running any of them, so a
//! mutually-recursive group declared in a plain `MODULE` seals and every member's name is visible
//! body-wide regardless of order.

use crate::builtins::test_support::{lookup_module, parse_one, TestRun};
use crate::machine::model::{AnnouncedData, NodeSchema, TypeDigest, TypeNode, TypeRegistry};
use crate::machine::model::{KExpression, KObject, KType};
use crate::machine::{program_storage, run_root_storage, KErrorKind, Scope};

/// `(scc-digest, scc-size, field-types)` of a sealed record-repr newtype member, read off its
/// `SetMember` identity. The SCC digest and component size witness which members sealed together;
/// the field types carry the absolute member handles the sealed schema references.
fn member_scc_and_fields(
    scope: &Scope<'_>,
    types: &TypeRegistry,
    name: &str,
) -> (TypeDigest, usize, Vec<(String, KType)>) {
    let handle = scope
        .resolve_type(name)
        .unwrap_or_else(|| panic!("expected {name} to be a type in scope"));
    match types.node(handle) {
        TypeNode::SetMember {
            scc_digest,
            scc_size,
            schema,
            ..
        } => match schema {
            NodeSchema::NewType(repr) => match types.node(repr) {
                TypeNode::Record { fields } => {
                    let fields = fields.iter().map(|(k, v)| (k.clone(), *v)).collect();
                    (scc_digest, scc_size, fields)
                }
                _ => panic!("expected {name} to carry a record repr, got {repr:?}"),
            },
            _ => panic!("expected {name} to carry a NewType schema for {handle:?}"),
        },
        _ => panic!("expected {name} to be a SetMember identity in types, got {handle:?}"),
    }
}

/// A mutually-recursive pair declared in a plain `MODULE` body seals into one shared component of
/// two, whichever order the declarations are written in: each cross-reference seals to the
/// referent's absolute handle, and both bind in the module's own scope.
#[test]
fn module_mutual_newtype_pair_seals_in_either_order() {
    for (first, second) in [("Aa", "Bb"), ("Bb", "Aa")] {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        let source = format!(
            "MODULE pair = (\n  NEWTYPE {first} = :{{other :{second}}}\n  \
             NEWTYPE {second} = :{{other :{first}}}\n)",
        );
        test_run.run(&source);
        let module = lookup_module(scope, "pair", &test_run.types);
        let members = module.child_scope();
        let types = test_run.types();
        let (a_scc, a_size, a_fields) = member_scc_and_fields(members, types, "Aa");
        let (b_scc, b_size, b_fields) = member_scc_and_fields(members, types, "Bb");
        assert_eq!(a_scc, b_scc, "Aa and Bb seal into one component");
        assert_eq!((a_size, b_size), (2, 2), "the group is a component of 2");
        let aa = members.resolve_type("Aa").expect("Aa binds");
        let bb = members.resolve_type("Bb").expect("Bb binds");
        assert_eq!(a_fields[0], ("other".to_string(), bb));
        assert_eq!(b_fields[0], ("other".to_string(), aa));
        assert!(
            members.bindings().pending_names().is_empty(),
            "a sealed group leaves no pending binding, got {:?}",
            members.bindings().pending_names(),
        );
    }
}

/// A `USING` window over the module surfaces the announced members as bare type names, which is how
/// a group declared inside a module is constructed at its use site.
///
/// Miri audit-slate: exercises the `AnnouncedWindow` the module child scope carries inline —
/// bumped member and binder runs written at the construction door, a fill run replaced with a
/// freshly bumped one by each declaration statement, the sealed runs bumped by the statement whose
/// fill closes the group, and every one of them read back from a later statement and from the
/// `USING` window over the finished module.
#[test]
fn announced_members_construct_through_using() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);
    test_run.run(
        "MODULE listy = (\n  \
           NEWTYPE Cell = :{head :Number, tail :Rest}\n  \
           NEWTYPE Rest = :{next :(Cell | Null)}\n\
         )\n\
         USING listy SCOPE (\n  \
           LET empty = (Rest {next = null})\n  \
           PRINT (Cell {head = 1, tail = empty})\n\
         )",
    );
    let printed = String::from_utf8(captured.borrow().clone()).expect("PRINT output is UTF-8");
    assert_eq!(printed, "Cell({head = 1, tail = Rest({next = null})})\n");
}

/// A `UNION` and a `NEWTYPE` that reference each other co-seal: the union's variants join the same
/// window as the newtype, so the cycle across the two declaration surfaces closes.
#[test]
fn module_union_newtype_cycle_seals() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);
    test_run.run(
        "MODULE t = (\n  \
           UNION Tree = (Leaf :Number, Node :Forest)\n  \
           NEWTYPE Forest = :(LIST OF Tree)\n\
         )\n\
         USING t SCOPE (PRINT (Tree (Node (Forest [(Tree (Leaf 1))]))))",
    );
    let printed = String::from_utf8(captured.borrow().clone()).expect("PRINT output is UTF-8");
    assert_eq!(printed, "Node(Forest([Leaf(1)]))\n");
}

/// Announcement is identity-neutral. A member that references no sibling digests independently, so
/// a co-declared type unifies with its standalone twin — including a union, whose variants are
/// digested by their bare tags with the owning binder as a non-digested sort tiebreak.
#[test]
fn co_declared_types_unify_with_their_standalone_twins() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE t = (\n  \
           NEWTYPE Distance = Number\n  \
           UNION Maybe = (Some :Number, None :Null)\n\
         )\n\
         NEWTYPE Distance = Number\n\
         UNION Maybe = (Some :Number, None :Null)",
    );
    let module = lookup_module(scope, "t", &test_run.types);
    let inside = module.child_scope();
    assert_eq!(
        inside.resolve_type("Distance"),
        scope.resolve_type("Distance"),
        "a co-declared newtype referencing no sibling digests like its standalone twin",
    );
    assert_eq!(
        inside.resolve_type("Maybe"),
        scope.resolve_type("Maybe"),
        "a module-hosted union digests like its standalone twin — the owner orders, never digests",
    );
}

/// Only *top-level* statements announce: the scan reads the body's own statement split and never
/// descends into a statement's slots, so a declaration nested inside one is invisible to it and
/// keeps ordinary dataflow order.
#[test]
fn only_top_level_statements_announce() {
    let program = program_storage();
    let top_level = parse_one(
        &program,
        "MODULE t = (\n  NEWTYPE Boxed = Number\n  LET n = 1\n)",
    );
    fn body<'a>(statement: &crate::machine::model::KExpression<'a>) -> KExpression<'a> {
        match statement.parts.last().expect("a body slot").value {
            crate::machine::model::ExpressionPart::Expression(body) => *body,
            other => panic!("expected a body slot, got {other:?}"),
        }
    }
    let announced = super::super::announce_type_members(&body(&top_level), "t")
        .expect("the scan succeeds")
        .expect("a top-level declaration announces");
    assert_eq!(
        announced.members,
        vec![("Boxed".to_string(), None)],
        "the top-level NEWTYPE announces; the LET does not",
    );

    // The same declaration written inside another statement's slot is not the body's own statement,
    // so the scan announces nothing at all.
    let nested = parse_one(
        &program,
        "MODULE t = (\n  LET n = (NEWTYPE Boxed = Number)\n)",
    );
    assert!(
        super::super::announce_type_members(&body(&nested), "t")
            .expect("the scan succeeds")
            .is_none(),
        "a declaration nested in a statement's slot announces nothing",
    );
}

/// Announcing a type perturbs no identity: a co-declared member that references no sibling seals to
/// the same handle its standalone twin does.
#[test]
fn an_announced_member_keeps_its_standalone_identity() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("MODULE t = (\n  NEWTYPE Boxed = Number\n  LET n = 1\n)");
    let module = lookup_module(scope, "t", &test_run.types);
    let announced = module
        .child_scope()
        .resolve_type("Boxed")
        .expect("the announced member binds");
    test_run.run("NEWTYPE Boxed = Number");
    assert_eq!(scope.resolve_type("Boxed"), Some(announced));
}

/// A consumer of an announced member is visible body-wide and waits for the seal — before *and*
/// after the declarations it names, since announcement is order-independent.
#[test]
fn consumer_in_body_waits_for_seal() {
    for (before, after) in [("LET Alias = Cell", ""), ("", "LET Alias = Cell")] {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        let source = format!(
            "MODULE t = (\n  {before}\n  NEWTYPE Cell = :{{tail :Rest}}\n  \
             NEWTYPE Rest = :{{next :Cell}}\n  {after}\n)",
        );
        test_run.run(&source);
        let module = lookup_module(scope, "t", &test_run.types);
        let members = module.child_scope();
        assert_eq!(
            members.resolve_type("Alias"),
            members.resolve_type("Cell"),
            "the consumer resolves to the member's sealed identity",
        );
    }
}

/// A consumer never observes a pre-seal relative handle: when the member it names has no
/// declaration left to wait on, the reference is a typed miss, not a hang and not a handle that
/// would silently fail to match at dispatch.
#[test]
fn consumer_of_a_dead_member_errors_without_hanging() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("MODULE t = (\n  NEWTYPE Cell = :{tail :Bogus}\n  LET Alias = Cell\n)");
    assert!(
        scope.bindings().data().get("t").is_none(),
        "a module whose member failed to seal binds nothing",
    );
    assert!(
        scope.bindings().pending_names().is_empty(),
        "the failed declaration leaks no pending binding, got {:?}",
        scope.bindings().pending_names(),
    );
}

/// Belt check: a window whose announced member no declaration ever fills is a typed `KError` at the
/// module's own finish, never a hang. Reached only when the scan and dispatch disagree about what a
/// statement declares, so it is driven directly.
#[test]
fn unfilled_announced_member_is_a_typed_error() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let mut announced = AnnouncedData::default();
    announced.announce("Ghost".to_string());
    let child = scope.alloc_child_under_module("ghosted".into(), Some(announced));
    assert!(
        super::super::unsealed_announcement_error(scope, "ghosted").is_none(),
        "a window-less scope owes nothing",
    );
    let error = super::super::unsealed_announcement_error(child, "ghosted")
        .expect("an unfilled announced member must error");
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg)
            if msg.contains("announced type `Ghost`") && msg.contains("never sealed")),
        "expected the unsealed-announcement shape error, got {error}",
    );
}

/// Two unions in one body may own the same bare tag: qualified lookup is scoped by the binder's own
/// member list, so `:(Graph Node)` and `:(Tree Node)` name different members and both seal.
#[test]
fn two_unions_may_share_a_tag() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);
    test_run.run(
        "MODULE t = (\n  \
           UNION Graph = (Node :Number, Edge :Number)\n  \
           UNION Tree = (Node :Str, Twig :Str)\n\
         )\n\
         USING t SCOPE (\n  PRINT (Graph (Node (1)))\n  PRINT (Tree (Node (\"x\")))\n)",
    );
    let printed = String::from_utf8(captured.borrow().clone()).expect("PRINT output is UTF-8");
    assert_eq!(printed, "Node(1)\nNode(x)\n");
}

/// A co-declared `NEWTYPE` may type a field by a sibling union's variant through the qualified
/// sigil, pre-seal: `:(Tree Node)` lowers against the announced window rather than sub-dispatching,
/// which would deadlock on the seal's own producer.
#[test]
fn sibling_schema_references_a_variant_by_qualified_sigil() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);
    test_run.run(
        "MODULE t = (\n  \
           UNION Tree = (Leaf :Number, Node :Number)\n  \
           NEWTYPE Handle = :{n :(Tree Node)}\n\
         )\n\
         USING t SCOPE (PRINT (Handle {n = (Tree (Node (2)))}))",
    );
    let printed = String::from_utf8(captured.borrow().clone()).expect("PRINT output is UTF-8");
    assert_eq!(printed, "Handle({n = Node(2)})\n");
}

/// A `GROUP` body inherits the announcement — a group *is* a module.
#[test]
fn group_body_announces_its_type_declarations() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "GROUP g FOLD LEFT = (\n  \
           NEWTYPE Cell = :{tail :Rest}\n  \
           NEWTYPE Rest = :{next :Cell}\n  \
           OP #(+) OVER Number = (left)\n\
         )",
    );
    let group = lookup_module(scope, "g", &test_run.types);
    let members = group.child_scope();
    let types = test_run.types();
    let (_, size, _) = member_scc_and_fields(members, types, "Cell");
    assert_eq!(size, 2, "the group body's pair seals into one component");
}

/// Announcement stays a module property: the program's own top level announces nothing, so the
/// first half of a mutually-recursive pair is an ordinary forward reference and the group takes the
/// module wrapper.
#[test]
fn top_level_cycle_requires_a_module_wrapper() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let error = test_run.run_one_err(parse_one(&program, "NEWTYPE Cell = :{tail :Rest}"));
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg)
            if msg.contains("unknown type name `Rest`")),
        "a top-level forward type reference is an unknown-name miss, got {error}",
    );
    assert!(
        scope.resolve_type("Cell").is_none(),
        "so `Cell` never binds"
    );
}

/// A module body may not declare one type name twice — the scan refuses before any statement runs.
#[test]
fn duplicate_announced_name_is_a_shape_error() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let error = test_run.run_one_err(parse_one(
        &program,
        "MODULE t = (\n  NEWTYPE Aa = Number\n  NEWTYPE Aa = Str\n)",
    ));
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg)
            if msg.contains("declares type `Aa` twice")),
        "expected the duplicate-declaration shape error, got {error}",
    );
}

/// A variant is not a standalone type: it never reaches `bindings.types`, so it is absent from the
/// module's type members and unreachable by bare name through a `USING` window.
#[test]
fn variants_are_not_module_type_members() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("MODULE t = (\n  UNION Maybe = (Some :Number, None :Null)\n  NEWTYPE Aa = Str\n)");
    let module = lookup_module(scope, "t", &test_run.types);
    let mut members: Vec<String> = module
        .child_scope()
        .bindings()
        .iter_types()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    members.sort();
    assert_eq!(
        members,
        vec!["Aa".to_string(), "Maybe".to_string()],
        "the binder and the standalone member bind; the variants do not",
    );
    assert!(matches!(scope.lookup("t"), Some(KObject::Module(_))));
}
