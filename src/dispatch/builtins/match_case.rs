use std::collections::HashMap;
use std::rc::Rc;

use crate::dispatch::runtime::{CallArena, RuntimeArena};
use crate::dispatch::runtime::{KError, KErrorKind};
use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::values::KObject;
use crate::dispatch::runtime::Scope;
use crate::execute::scheduler::substitute_params;
use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

use super::helpers::extract_kexpression;
use super::{err, register_builtin};

/// `MATCH <value:Any> WITH <branches:KExpression>` — branch by tag.
///
/// `value` may be a `Tagged` (user-defined tagged union) or a `Bool`. For `Bool`, the
/// value is projected at entry into a synthetic `(tag, value)` pair where `tag` is
/// `"true"` or `"false"` and the inner value is `Null`; the rest of the branch-walking
/// machinery is the same path used by `Tagged`. Other input types are a `TypeMismatch`.
///
/// `branches` is the parens-wrapped body whose parts are repeated `<tag> -> <body>`
/// triples (arrow-pair syntax). The tag part is normally a bare identifier; for `Bool`
/// matches it is the literal `true` or `false` (which the parser tokenizes as
/// `KLiteral::Boolean`, accepted here in the same position). The body of the first
/// branch whose tag matches `value.tag` is dispatched as a tail expression; the others
/// are never touched. `it` is bound to the inner value in a per-MATCH child scope
/// (and substituted into Identifier-typed positions of the body), modeled on
/// `KFunction::invoke`'s per-call frame so the binding doesn't leak into the
/// surrounding scope. For `Bool` matches `it` is `Null` — accurate, since there is no
/// payload.
///
/// No matching branch → `ShapeError("inexhaustive match: no branch for `X`")` — same
/// rule for `Bool` as for `Tagged`, so `MATCH cond WITH (true -> ...)` against a
/// `false` value is an error rather than a silent null. Malformed branch shape (not
/// `<tag> -> <body>` triples) → `ShapeError`.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let (tag, value) = match bundle.get("value") {
        Some(KObject::Tagged { tag, value }) => (tag.clone(), Rc::clone(value)),
        Some(KObject::Bool(b)) => (
            if *b { "true".to_string() } else { "false".to_string() },
            Rc::new(KObject::Null),
        ),
        Some(other) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "value".to_string(),
                expected: "Tagged or Bool".to_string(),
                got: other.ktype().name().to_string(),
            }));
        }
        None => return err(KError::new(KErrorKind::MissingArg("value".to_string()))),
    };
    let branches_expr = match extract_kexpression(&mut bundle, "branches") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "MATCH branches slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    let branch_body = match find_branch_body(&branches_expr, &tag) {
        Ok(Some(body)) => body,
        Ok(None) => {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "inexhaustive match: no branch for `{}`",
                tag
            ))));
        }
        Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
    };
    // Per-MATCH frame for the `it` binding — same pattern as `KFunction::invoke`. The
    // child scope's `outer` is the MATCH call site, so free names in the branch body
    // resolve against the surrounding scope. `it` is bound only in the child, so it
    // disappears when the frame drops. The call-site frame Rc is chained onto the new
    // frame's `outer_frame`: the call-site scope lives in *some* arena, and if that arena
    // is per-call (e.g., MATCH inside a user-fn body), it must stay alive while the new
    // frame's child scope's `outer` pointer is in use. `current_frame` returns `None` when
    // the call site is run-root (which outlives the run, so no chain needed).
    let frame: Rc<CallArena> = CallArena::new(scope, sched.current_frame());
    let arena_ptr: *const RuntimeArena = frame.arena();
    let scope_ptr: *const Scope<'_> = frame.scope();
    // SAFETY: heap-pinning makes both pointers valid for the Rc's lifetime. The
    // re-borrow ends before the `frame` move into `BodyResult::Tail`.
    let inner_arena: &'a RuntimeArena = unsafe { &*(arena_ptr as *const _) };
    let child: &'a Scope<'a> = unsafe { &*(scope_ptr as *const _) };
    let it_obj: &'a KObject<'a> = inner_arena.alloc_object(value.deep_clone());
    child.add("it".to_string(), it_obj);
    let mut it_bundle = ArgumentBundle { args: HashMap::new() };
    it_bundle.args.insert("it".to_string(), Rc::new(value.deep_clone()));
    let substituted = substitute_params(branch_body, &it_bundle, inner_arena);
    // Construct the Tail variant directly. `tail_with_frame` requires a `&KFunction` for
    // return-type enforcement and error-frame attribution; MATCH has no meaningful
    // function to attach (declared return is `Any`, so the check would be a no-op).
    BodyResult::Tail { expr: substituted, frame: Some(frame), function: None }
}

/// Walk the branches KExpression's parts as repeated `<Identifier(t)> <Keyword("->")>
/// <Expression(body)>` triples. Return the body for the first triple whose tag matches
/// `target_tag`, `Ok(None)` if no triple matches, or `Err` on shape mismatch.
fn find_branch_body<'a>(
    branches: &KExpression<'a>,
    target_tag: &str,
) -> Result<Option<KExpression<'a>>, String> {
    let parts = &branches.parts;
    if !parts.len().is_multiple_of(3) {
        return Err(format!(
            "MATCH branches must be `<tag> -> <body>` triples; got {} parts (not a multiple of 3)",
            parts.len()
        ));
    }
    let mut i = 0;
    while i < parts.len() {
        let tag_part = &parts[i];
        let arrow_part = &parts[i + 1];
        let body_part = &parts[i + 2];
        let tag_name = match tag_part {
            ExpressionPart::Identifier(s) => s.clone(),
            // `true`/`false` are `KLiteral::Boolean` from the parser, not identifiers,
            // but they're the natural tag form for `MATCH` on a `Bool` value. Accept
            // them here so users can write `(true -> ... false -> ...)` directly.
            ExpressionPart::Literal(KLiteral::Boolean(b)) => {
                if *b { "true".to_string() } else { "false".to_string() }
            }
            other => {
                return Err(format!(
                    "MATCH branch tag must be a bare identifier or boolean literal, got {}",
                    other.summarize()
                ));
            }
        };
        match arrow_part {
            ExpressionPart::Keyword(k) if k == "->" => {}
            other => {
                return Err(format!(
                    "MATCH branch separator must be `->`, got {}",
                    other.summarize()
                ));
            }
        }
        let body_expr = match body_part {
            ExpressionPart::Expression(e) => (**e).clone(),
            other => {
                return Err(format!(
                    "MATCH branch body must be a parenthesized expression, got {}",
                    other.summarize()
                ));
            }
        };
        if tag_name == target_tag {
            return Ok(Some(body_expr));
        }
        i += 3;
    }
    Ok(None)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "MATCH",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("MATCH".into()),
                SignatureElement::Argument(Argument { name: "value".into(),    ktype: KType::Any }),
                SignatureElement::Keyword("WITH".into()),
                SignatureElement::Argument(Argument { name: "branches".into(), ktype: KType::KExpression }),
            ],
        },
        body,
    );
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    use crate::dispatch::runtime::RuntimeArena;
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::runtime::KErrorKind;
    use crate::dispatch::runtime::Scope;
    use crate::execute::scheduler::Scheduler;
    use crate::parse::expression_tree::parse;
    use crate::parse::kexpression::KExpression;

    struct SharedBuf(Rc<RefCell<Vec<u8>>>);
    impl Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    fn build_scope<'a>(arena: &'a RuntimeArena, captured: Rc<RefCell<Vec<u8>>>) -> &'a Scope<'a> {
        default_scope(arena, Box::new(SharedBuf(captured)))
    }

    fn parse_one(src: &str) -> KExpression<'static> {
        let mut exprs = parse(src).expect("parse should succeed");
        assert_eq!(exprs.len(), 1, "test helper expects a single expression");
        exprs.remove(0)
    }

    fn run<'a>(scope: &'a Scope<'a>, source: &str) {
        let exprs = parse(source).expect("parse should succeed");
        let mut sched = Scheduler::new();
        for expr in exprs {
            sched.add_dispatch(expr, scope);
        }
        sched.execute().expect("scheduler should succeed");
    }

    fn run_one_err<'a>(
        scope: &'a Scope<'a>,
        expr: KExpression<'a>,
    ) -> crate::dispatch::runtime::KError {
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().expect("scheduler should not surface errors directly");
        match sched.read_result(id) {
            Ok(_) => panic!("expected error"),
            Err(e) => e.clone(),
        }
    }

    fn run_program(source: &str) -> Vec<u8> {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured.clone());
        run(scope, source);
        let bytes = captured.borrow().clone();
        bytes
    }

    #[test]
    fn match_dispatches_branch_for_matching_tag() {
        let bytes = run_program(
            "UNION Maybe = (some: Number none: Null)\n\
             LET m = (Maybe (some 42))\n\
             MATCH (m) WITH (some -> (PRINT \"got\") none -> (PRINT \"no\"))",
        );
        assert_eq!(bytes, b"got\n");
    }

    #[test]
    fn match_binds_inner_value_to_it() {
        // `it` is substituted into Identifier-typed positions; here PRINT's `msg:Str` slot
        // wants a Str literal or Future, and substitution rewrites the `it` Identifier into
        // a `Future(value)` so the bind succeeds.
        let bytes = run_program(
            "UNION Result = (ok: Str err: Str)\n\
             LET r = (Result (ok \"all good\"))\n\
             MATCH (r) WITH (ok -> (PRINT it) err -> (PRINT \"failed\"))",
        );
        assert_eq!(bytes, b"all good\n");
    }

    #[test]
    fn match_does_not_run_unmatched_branches() {
        // Lazy: the `none` branch's PRINT must not fire when the value is `some`.
        let bytes = run_program(
            "UNION Maybe = (some: Number none: Null)\n\
             LET m = (Maybe (some 1))\n\
             MATCH (m) WITH (some -> (PRINT \"yes\") none -> (PRINT \"NO_SHOULD_NOT_APPEAR\"))",
        );
        assert_eq!(bytes, b"yes\n");
    }

    #[test]
    fn match_inexhaustive_errors() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(
            scope,
            "UNION Maybe = (some: Number none: Null)\nLET m = (Maybe (none null))",
        );
        let err = run_one_err(scope, parse_one("MATCH (m) WITH (some -> (PRINT \"yes\"))"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("inexhaustive") && msg.contains("`none`")),
            "expected inexhaustive ShapeError, got {err}",
        );
    }

    #[test]
    fn match_other_branch_runs_when_tag_matches() {
        let bytes = run_program(
            "UNION Maybe = (some: Number none: Null)\n\
             LET m = (Maybe (none null))\n\
             MATCH (m) WITH (some -> (PRINT \"yes\") none -> (PRINT \"nothing\"))",
        );
        assert_eq!(bytes, b"nothing\n");
    }

    #[test]
    fn match_on_bool_true_takes_true_branch() {
        let bytes = run_program(
            "MATCH true WITH (true -> (PRINT \"yes\") false -> (PRINT \"no\"))",
        );
        assert_eq!(bytes, b"yes\n");
    }

    #[test]
    fn match_on_bool_false_takes_false_branch() {
        let bytes = run_program(
            "MATCH false WITH (true -> (PRINT \"yes\") false -> (PRINT \"no\"))",
        );
        assert_eq!(bytes, b"no\n");
    }

    #[test]
    fn recursive_tagged_match_no_uaf() {
        // Regression: a recursive HOP through a tagged value triggered a use-after-free
        // during writer drop. Root cause was structural in the scheduler/MATCH frame
        // chain: MATCH built a per-call `CallArena` whose child scope's `outer` pointed
        // into the call-site (the per-call arena of the enclosing user-fn). The
        // enclosing-fn frame was dropped on TCO replace before MATCH's deferred lift
        // ran, so the value-lift read `scope.outer.arena` through a freed pointer.
        // Fixed by chaining the call-site frame's Rc onto the new `CallArena` via
        // `SchedulerHandle::current_frame` + `outer_frame`.
        let bytes = run_program(
            "UNION Bit = (one: Null zero: Null)\n\
             FN (HOP b: Tagged) -> Any = (MATCH (b) WITH (\
                 one -> (HOP (Bit (zero null)))\
                 zero -> (PRINT \"done\")\
             ))\n\
             HOP (Bit (one null))",
        );
        assert_eq!(bytes, b"done\n");
    }

    #[test]
    fn match_on_bool_inexhaustive_errors() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let err = run_one_err(scope, parse_one("MATCH true WITH (false -> (PRINT \"x\"))"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("inexhaustive") && msg.contains("`true`")),
            "expected inexhaustive ShapeError for missing true branch, got {err}",
        );
    }
}
