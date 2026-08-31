//! Explicit laziness: a bare `(…)` argument evaluates before its parent dispatches everywhere
//! except a lazy slot of a fixed builtin form, so a user signature receives code only as a
//! `KExpression` *value*.

use crate::builtins::test_support::TestRun;
use crate::machine::KErrorKind;
use crate::machine::{program_storage, run_root_storage};

/// A program's `PRINT` output. Each line is one evaluation, so a body that prints counts its own
/// runs.
fn run_program(source: &str) -> Vec<u8> {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);
    test_run.run(source);
    captured.borrow().clone()
}

/// A `#(…)` literal satisfies a user `:KExpression` parameter, and the body receives the
/// expression itself — the quote is the spelling that hands code to a user form.
#[test]
fn a_quote_satisfies_a_user_kexpression_parameter() {
    let bytes = run_program(
        "FN (RUNLATER body :KExpression) -> Any = ($(body))\n\
         PRINT (RUNLATER #(1 + 2))",
    );
    assert_eq!(bytes, b"3\n");
}

/// A name bound to a quote satisfies it too: the slot is an ordinary eager value parameter, so any
/// `KExpression`-valued expression fills it — not just a literal quote.
#[test]
fn a_name_bound_to_a_quote_satisfies_a_kexpression_parameter() {
    let bytes = run_program(
        "FN (RUNLATER body :KExpression) -> Any = ($(body))\n\
         LET quoted = #(9 + 1)\n\
         PRINT (RUNLATER quoted)",
    );
    assert_eq!(bytes, b"10\n");
}

/// So does a bare group that *evaluates* to code — the slot classifies the landed value, not the
/// spelling that produced it.
#[test]
fn a_group_evaluating_to_code_satisfies_a_kexpression_parameter() {
    let bytes = run_program(
        "FN (SAME q :KExpression) -> Any = (q)\n\
         FN (RUNLATER body :KExpression) -> Any = ($(body))\n\
         PRINT (RUNLATER (SAME #(9 + 1)))",
    );
    assert_eq!(bytes, b"10\n");
}

/// A bare group in a `:KExpression` slot is not code: it evaluates — its side effects run — and
/// only then does dispatch miss. The error names the quote as the fix, because that failure mode
/// is worth pointing at.
#[test]
fn a_bare_group_in_a_kexpression_parameter_evaluates_then_misses() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);
    test_run.run("FN (RUNLATER body :KExpression) -> Any = ($(body))");
    let error = test_run.run_one_err(test_run.parse_one("RUNLATER (PRINT \"side effect\")"));
    assert_eq!(
        captured.borrow().clone(),
        b"side effect\n",
        "the argument evaluates before dispatch, so its side effect happens exactly once",
    );
    let KErrorKind::DispatchFailed { reason, .. } = &error.kind else {
        panic!("expected a dispatch miss, got {error}");
    };
    assert!(
        reason.contains("#(…)"),
        "the miss names the quote as the likely fix, got {reason}",
    );
}

/// The same rule in an ordinary typed slot: the group evaluates exactly once, before its parent
/// dispatches. One `side` line, from the one evaluation.
#[test]
fn a_bare_group_argument_evaluates_exactly_once_before_dispatch() {
    let bytes = run_program(
        "FN (TAKE s :Str) -> Str = (\"done\")\n\
         PRINT (TAKE (PRINT \"side\"))",
    );
    assert_eq!(bytes, b"side\ndone\n");
}

/// A builtin's lazy slots are stamped at seal, so a quoted `MATCH` spliced back in by `EVAL` still
/// runs only its selected branch: the stamp travelled with the quoted node.
#[test]
fn an_eval_spliced_quote_keeps_its_builtin_lazy_branches() {
    let bytes = run_program(
        "UNION Maybe = (Some :Number None :Null)\n\
         LET m = (Maybe (Some 1))\n\
         LET branch = #(MATCH (m) -> :Str WITH \
             (Some -> (PRINT \"yes\") None -> (PRINT \"NO_SHOULD_NOT_APPEAR\")))\n\
         $(branch)",
    );
    assert_eq!(bytes, b"yes\n");
}

/// A builtin's lazy slot takes a `KExpression` *value* too. The stamp keeps a `(…)` or `#(…)` part
/// there raw — those two spellings of the body stay one — but a bare name is neither raw nor
/// staged, so it resolves and the slot classifies the code it carries. One rule for the slot type
/// everywhere: a `:KExpression` slot takes code, however the code reached it. The builtin receives
/// the same node either way.
#[test]
fn a_name_bound_to_a_quote_fills_a_builtin_lazy_slot() {
    let bytes = run_program(
        "UNION Maybe = (Some :Number None :Null)\n\
         LET m = (Maybe (Some 1))\n\
         LET branches = #(Some -> (PRINT \"yes\") None -> (PRINT \"NO_SHOULD_NOT_APPEAR\"))\n\
         MATCH (m) -> :Str WITH branches",
    );
    assert_eq!(bytes, b"yes\n");
}

/// The miss names each argument slot by the type dispatch matched it on, not by the value it
/// evaluated to. The error carries the offending expression's site, so the spelling is recoverable
/// from source without the message echoing it.
#[test]
fn a_missed_dispatch_renders_an_evaluated_argument_by_type() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, _captured) = TestRun::with_buf(&program, &region);
    test_run.run("FN (RUNLATER body :KExpression) -> Any = ($(body))");
    let error = test_run.run_one_err(test_run.parse_one("RUNLATER (1 + 2)"));
    let KErrorKind::DispatchFailed { expr, .. } = &error.kind else {
        panic!("expected a dispatch miss, got {error}");
    };
    assert_eq!(expr, "RUNLATER Number");
}

/// The rendering tracks the dispatch axis and nothing else: two values of one type read alike,
/// because dispatch cannot tell them apart either, while two types read apart.
#[test]
fn the_rendering_tracks_the_type_not_the_value() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, _captured) = TestRun::with_buf(&program, &region);
    test_run.run("FN (RUNLATER body :KExpression) -> Any = ($(body))");
    let mut render = |source: &str| {
        let error = test_run.run_one_err(test_run.parse_one(source));
        let KErrorKind::DispatchFailed { expr, .. } = &error.kind else {
            panic!("expected a dispatch miss, got {error}");
        };
        expr.clone()
    };
    assert_eq!(render("RUNLATER (1 + 2)"), "RUNLATER Number");
    assert_eq!(
        render("RUNLATER (2 + 2)"),
        "RUNLATER Number",
        "two values of one type share a rendering — dispatch cannot tell them apart either",
    );
    assert_eq!(render("RUNLATER (\"hi\")"), "RUNLATER Str");
    assert_eq!(render("RUNLATER ([1 2 3])"), "RUNLATER :(LIST OF Number)");
}

/// A slot renders the same whether or not it happened to evaluate: an unevaluated literal and a
/// group evaluating to the same type read alike, because dispatch matched them on the same type.
/// The evaluated/unevaluated split is a scheduling detail, not something a diagnostic reports.
#[test]
fn an_evaluated_slot_and_an_unevaluated_one_of_a_type_read_alike() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, _captured) = TestRun::with_buf(&program, &region);
    test_run.run("FN (RUNLATER body :KExpression) -> Any = ($(body))");
    let mut render = |source: &str| {
        let error = test_run.run_one_err(test_run.parse_one(source));
        let KErrorKind::DispatchFailed { expr, .. } = &error.kind else {
            panic!("expected a dispatch miss, got {error}");
        };
        expr.clone()
    };
    assert_eq!(render("RUNLATER 3"), render("RUNLATER (1 + 2)"));
    assert_eq!(render("RUNLATER 3"), "RUNLATER Number");
    // A raw type token and a group evaluating to a type both denote a proper type.
    assert_eq!(render("RUNLATER Number"), render("RUNLATER (Number)"));
    assert_eq!(render("RUNLATER Number"), "RUNLATER ProperType");
}

/// A keyword fills no slot, so it keeps its own spelling — the rendering names types only where
/// dispatch was matching one.
#[test]
fn a_keyword_keeps_its_spelling() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, _captured) = TestRun::with_buf(&program, &region);
    test_run.run("FN (RUNLATER body :KExpression) -> Any = ($(body))");
    let error = test_run.run_one_err(test_run.parse_one("RUNLATER (1 + 2)"));
    let KErrorKind::DispatchFailed { expr, .. } = &error.kind else {
        panic!("expected a dispatch miss, got {error}");
    };
    assert!(
        expr.starts_with("RUNLATER "),
        "the dispatch keyword renders as itself, got {expr}",
    );
}

/// A binder name slot is not a dispatch axis — it is the name being declared — so it keeps its
/// spelling even once the name already resolves in scope. The miss path's resolution splice skips
/// that slot for exactly this reason; splicing there would render the bound value's type in place
/// of the name.
#[test]
fn a_binder_name_keeps_its_spelling_once_bound() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, _captured) = TestRun::with_buf(&program, &region);
    test_run.run("LET greet = \"hi\"");
    let error = test_run.run_one_err(test_run.parse_one("MODULE greet = 5"));
    let KErrorKind::DispatchFailed { expr, .. } = &error.kind else {
        panic!("expected a dispatch miss, got {error}");
    };
    assert_eq!(
        expr, "MODULE greet = Number",
        "the declared name spells itself; only the value slot names a type",
    );
}

/// A miss carries the offending expression's site itself, not only via an enclosing call's trace
/// frame — so a top-level statement, which nothing encloses, still locates. That is what pays for
/// the summary naming types instead of echoing the source.
#[test]
fn a_top_level_miss_carries_its_own_location() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, _captured) = TestRun::with_buf(&program, &region);
    test_run.run("FN (RUNLATER body :KExpression) -> Any = ($(body))");
    let error = test_run.run_one_err(test_run.parse_one("RUNLATER (\"hi\")"));
    assert!(
        error.frames.is_empty(),
        "nothing encloses a top-level statement"
    );
    let KErrorKind::DispatchFailed { location, .. } = &error.kind else {
        panic!("expected a dispatch miss, got {error}");
    };
    let location = location.as_ref().expect("a parsed statement has a site");
    assert!(
        format!("{error}").contains(&format!(
            "at {}:{}:{}",
            location.path, location.line, location.col_utf16
        )),
        "the site renders into the message, got {error}",
    );
}
