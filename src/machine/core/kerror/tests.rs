//! `Display`-rendering round-trip per `KErrorKind` variant. Pins format strings against
//! accidental rewording — if you change a message, update the matching test here.
use super::*;

fn render(kind: KErrorKind) -> String {
    format!("{}", KError::new(kind))
}

#[test]
fn display_type_mismatch() {
    let s = render(KErrorKind::TypeMismatch {
        arg: "x".into(),
        expected: "Number".into(),
        got: "Str".into(),
    });
    assert_eq!(
        s,
        "type mismatch for argument 'x': expected Number, got Str"
    );
}

#[test]
fn display_missing_arg() {
    assert_eq!(
        render(KErrorKind::MissingArg("y".into())),
        "missing argument 'y'"
    );
}

#[test]
fn display_unbound_name() {
    assert_eq!(
        render(KErrorKind::UnboundName("foo".into())),
        "unbound name 'foo'"
    );
}

#[test]
fn display_arity_mismatch() {
    let s = render(KErrorKind::ArityMismatch {
        expected: 2,
        got: 3,
    });
    assert_eq!(s, "arity mismatch = expected 2 arguments, got 3");
}

#[test]
fn display_ambiguous_dispatch() {
    let s = render(KErrorKind::AmbiguousDispatch {
        expr: "(F 1)".into(),
        candidates: 2,
    });
    assert_eq!(
        s,
        "ambiguous dispatch: 2 candidates match (F 1) with equal specificity"
    );
}

#[test]
fn display_dispatch_failed() {
    let s = render(KErrorKind::DispatchFailed {
        expr: "(G 1)".into(),
        reason: "no overload accepts Number".into(),
    });
    assert_eq!(s, "dispatch failed for (G 1): no overload accepts Number");
}

#[test]
fn display_shape_error() {
    assert_eq!(
        render(KErrorKind::ShapeError("bad parts".into())),
        "shape error: bad parts"
    );
}

#[test]
fn display_parse_error_without_location() {
    let kind = KErrorKind::ParseError {
        message: "eof".into(),
        span: None,
        file: None,
    };
    assert_eq!(render(kind), "parse error: eof");
}

#[test]
fn display_parse_error_with_location_renders_path_line_col() {
    let id = source::register(source::SourceFile::new("<t>", "a\nbcd".to_string()));
    let kind = KErrorKind::ParseError {
        message: "bad token".into(),
        span: Some(Span { start: 3, end: 4 }),
        file: Some(id),
    };
    assert_eq!(render(kind), "parse error at <t>:2:2: bad token");
}

#[test]
fn display_user_message_is_verbatim() {
    assert_eq!(render(KErrorKind::User("boom".into())), "boom");
}

#[test]
fn display_rebind() {
    let s = render(KErrorKind::Rebind { name: "x".into() });
    assert_eq!(s, "name 'x' is already bound in this scope");
}

#[test]
fn display_duplicate_overload() {
    let s = render(KErrorKind::DuplicateOverload {
        name: "F".into(),
        signature: "(Number)".into(),
    });
    assert_eq!(
        s,
        "function 'F' already has an overload with signature (Number)"
    );
}

#[test]
fn display_type_class_binding_expects_type() {
    let s = render(KErrorKind::TypeClassBindingExpectsType {
        name: "T".into(),
        got: "Number".into(),
    });
    assert_eq!(
        s,
        "type-class binding `T` expects a type value, got `Number`"
    );
}

#[test]
fn with_frame_renders_call_stack_inline() {
    let err = KError::new(KErrorKind::User("boom".into()))
        .with_frame(Frame::bare("F", "(F 1)"))
        .with_frame(Frame::bare("G", "(G (F 1))"));
    assert_eq!(err.to_string(), "boom\n  in (F 1) (F)\n  in (G (F 1)) (G)");
}

#[test]
fn frame_with_location_appends_path_line_col() {
    let loc = SourceLoc {
        path: "lib.koan".into(),
        line: 4,
        col_utf16: 7,
    };
    let err = KError::new(KErrorKind::User("boom".into())).with_frame(Frame {
        function: "F".into(),
        expression: "(F 1)".into(),
        location: Some(loc),
    });
    assert_eq!(err.to_string(), "boom\n  in (F 1) (F) at lib.koan:4:7");
}

#[test]
fn debug_matches_display() {
    let err = KError::new(KErrorKind::MissingArg("z".into())).with_frame(Frame::bare("F", "(F)"));
    assert_eq!(format!("{:?}", err), format!("{}", err));
}

#[test]
fn clone_for_propagation_preserves_kind_and_frames() {
    let err =
        KError::new(KErrorKind::UnboundName("q".into())).with_frame(Frame::bare("H", "(H q)"));
    let copy = err.clone_for_propagation();
    assert_eq!(copy.to_string(), err.to_string());
    assert_eq!(copy.frames.len(), 1);
}
