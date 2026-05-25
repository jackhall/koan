//! `errors` interpret/execute integration tests.


use super::*;
use crate::machine::KErrorKind;
use crate::machine::execute::interpret_with_writer_path;


/// A bare unbound name at the top level surfaces as `KError::UnboundName` rather than
/// the prior silent `KObject::Null` swallow.
#[test]
fn unbound_name_at_top_level_returns_error() {
    let result = interpret_with_writer("foo", Box::new(std::io::sink()));
    match result {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::UnboundName(name) if name == "foo"),
            "expected UnboundName(\"foo\"), got {e}",
        ),
        Ok(()) => panic!("expected UnboundName error, got Ok"),
    }
}

/// An error inside a user-fn body carries at least one `Frame` whose function field
/// names the user-fn — proving the call-stack trace works through user invocation.
#[test]
fn error_inside_user_fn_body_carries_frame() {
    let result = interpret_with_writer(
        "FN (BAD) -> Any = (undefined_thing)\nBAD",
        Box::new(std::io::sink()),
    );
    match result {
        Err(e) => {
            assert!(
                matches!(&e.kind, KErrorKind::UnboundName(name) if name == "undefined_thing"),
                "expected UnboundName(\"undefined_thing\"), got {e}",
            );
            assert!(
                e.frames.iter().any(|f| f.function.contains("BAD")),
                "expected a frame mentioning BAD, got frames: {:?}",
                e.frames.iter().map(|f| &f.function).collect::<Vec<_>>(),
            );
        }
        Ok(()) => panic!("expected error from undefined name in user-fn body"),
    }
}

/// The first errored top-level expression short-circuits the program's reported
/// outcome; subsequent top-level dispatches still run (Scheduler::execute keeps
/// draining the queue), but interpret returns the first error and any later bindings
/// are observable side-effects rather than program-level "success."
#[test]
fn error_short_circuits_program_outcome() {
    let result = interpret_with_writer("undefined\nLET y = 5", Box::new(std::io::sink()));
    match result {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::UnboundName(name) if name == "undefined"),
            "expected UnboundName(\"undefined\") to be the surfaced error, got {e}",
        ),
        Ok(()) => panic!("expected first-line error to short-circuit interpret's outcome"),
    }
}

/// A made-up function call with no matching signature surfaces as
/// `KError::DispatchFailed`. (`WAT THIS IS NOT FUNC` parses as a multi-token expression
/// with ≥2-uppercase keyword tokens, so dispatch fails to find a match.)
#[test]
fn dispatch_failure_surfaces_as_kerror() {
    let result = interpret_with_writer(
        "WAT THIS IS NOT FUNC",
        Box::new(std::io::sink()),
    );
    match result {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed, got {e}",
        ),
        Ok(()) => panic!("expected dispatch failure for unmatched expression"),
    }
}

/// A type-mismatched argument that fits the bucket shape but fails dispatch's
/// per-slot type check surfaces as `KError::DispatchFailed` (no overload matches).
/// `MATCH` requires `branches: KExpression`; passing a string literal in that slot
/// fits the bucket shape (4 parts: `MATCH _ WITH _`) but fails the slot-type check,
/// so dispatch finds zero candidates. Type mismatches that DO reach `bind` (only
/// possible with an overload set richer than today's) would surface as
/// `TypeMismatch` from the bind step.
#[test]
fn type_mismatch_at_dispatch_surfaces_as_dispatch_failed() {
    let result = interpret_with_writer(
        "MATCH true WITH \"not_an_expression\"",
        Box::new(std::io::sink()),
    );
    match result {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed for unmatchable MATCH call, got {e}",
        ),
        Ok(()) => panic!("expected dispatch failure on MATCH with non-KExpression branches"),
    }
}

/// Parse errors carry a span + file when source is registered via `parse_with_path`.
/// `Display` then renders the `parse error at <path>:<line>:<col>: <message>` shape.
/// Tab-indented continuation hits `collapse_whitespace`'s tab rejection.
#[test]
fn parse_error_carries_span_and_renders_location() {
    let result = interpret_with_writer_path(
        "(foo)\n\t(bar)",
        Some("script.koan"),
        Box::new(std::io::sink()),
    );
    match result {
        Err(e) => {
            match &e.kind {
                KErrorKind::ParseError { span, file, .. } => {
                    assert!(span.is_some(), "expected span on parse error: {e}");
                    assert!(file.is_some(), "expected file on parse error: {e}");
                }
                _ => panic!("expected ParseError, got {e}"),
            }
            let rendered = e.to_string();
            assert!(
                rendered.contains(" at script.koan:"),
                "expected ' at script.koan:' in rendered output, got {rendered}",
            );
        }
        Ok(()) => panic!("expected parse error on tab-indented source"),
    }
}

/// Frame chain walks user-fn calls: an error in INNER, called from OUTER (via a
/// non-tail position so OUTER's frame survives), surfaces with frames listing both
/// function names. OUTER's body wraps INNER's call inside a `LET xx = (INNER)` so
/// the body has 4 parts (not a single-Expression wrapper that the parser would peel)
/// and INNER becomes a sub-Dispatch within OUTER's body — OUTER's slot then defers
/// to a `Lift` shim holding OUTER's frame, and the Done arm appends OUTER's frame
/// as the propagated Err lands on the slot. Direct `((INNER))` would peel to `INNER`
/// and tail-call into INNER, causing TCO to replace OUTER's frame with INNER's.
#[test]
fn frame_chain_walks_nested_user_fn_calls() {
    let result = interpret_with_writer(
        "FN (INNER) -> Any = (undefined)\n\
         FN (OUTER) -> Any = (LET xx = (INNER))\n\
         OUTER",
        Box::new(std::io::sink()),
    );
    match result {
        Err(e) => {
            let frame_names: Vec<String> =
                e.frames.iter().map(|f| f.function.clone()).collect();
            assert!(
                frame_names.iter().any(|n| n.contains("INNER")),
                "expected a frame mentioning INNER, got {:?} (full error: {})",
                frame_names,
                e,
            );
            assert!(
                frame_names.iter().any(|n| n.contains("OUTER")),
                "expected a frame mentioning OUTER, got {:?} (full error: {})",
                frame_names,
                e,
            );
        }
        Ok(()) => panic!("expected error from undefined name in INNER"),
    }
}
