//! A construction's static type: a newtype's, a family's, and a union variant's through the path
//! that reaches it.

use super::run;
use super::statics::{ends, interval, loaded, slot};

const MAYBE: &str = "UNION Maybe = #{Some: Number, None: Null}";
const OPTION: &str = "UNION (Elem -> Option) = #{Some: Elem, None: Null}";
const TREE: &str = "UNION (Elem -> Tree) = #{Leaf: Null, Node: :{value :Elem, left :(Elem -> Tree), right :(Elem -> Tree)}}";
const SHAPE: &str = "UNION Shape = #{Sized: Maybe, Flat: Null}";

/// The static type of the top-level binder `v` of `source`, rendered as its two ends.
fn constructed(source: &str) -> String {
    loaded(source, |program| {
        ends(program, interval(program, program.shape(), "v"))
    })
}

#[test]
fn a_construction_through_a_union_lies_between_its_variant_and_the_union() {
    for (body, expected) in [
        ("LET v = (Maybe.Some 3)", "[Some, :(Some | None)]"),
        ("LET v = (:(Maybe.Some) 3)", "[Some, :(Some | None)]"),
        (
            "LET Alias = Maybe\nLET v = (Alias.Some 7)",
            "[Some, :(Some | None)]",
        ),
        ("LET v = (Maybe.Some \"s\")", "[Never, :(Some | None)]"),
    ] {
        assert_eq!(constructed(&format!("{MAYBE}\n{body}")), expected, "{body}");
    }
    assert_eq!(
        constructed("MODULE m = (UNION Mb = #{Some: Number, None: Null})\nLET v = (m.Mb.Some 3)"),
        "[Some, :(Some | None)]",
        "a module path reaches its union"
    );
}

#[test]
fn a_construction_through_no_union_is_its_variant() {
    assert_eq!(
        constructed(&format!("{MAYBE}\nLET Sm = Maybe.Some\nLET v = (Sm 3)")),
        "Some",
        "a variant alias spells no union"
    );
    assert_eq!(
        constructed("NEWTYPE Some = Number\nLET v = (Some 4)"),
        "Some"
    );
}

#[test]
fn a_family_construction_is_at_most_its_union_applied_at_the_payload() {
    assert_eq!(
        constructed(&format!("{OPTION}\nLET v = (Option.Some 3)")),
        "[:(Some {Elem = Number}), :((Some {Elem = Number}) | (None {Elem = Number}))]"
    );
    assert_eq!(
        constructed(&format!("{TREE}\nLET v = (Tree.Leaf null)")),
        "[:(Leaf {Elem = Never}), :((Leaf {Elem = Never}) | (Node {Elem = Never}))]",
        "a parameter the payload says nothing about is `Never`"
    );
}

#[test]
fn a_tag_projection_reads_through_a_variant_s_payload() {
    assert_eq!(
        constructed(&format!("{MAYBE}\n{SHAPE}\nLET v = (Shape.Sized.Some 3)")),
        "[Some, :(Some | None)]"
    );
    assert_eq!(
        constructed(&format!(
            "{MAYBE}\n{SHAPE}\nLET v = (Shape.Sized (Maybe.Some 3))"
        )),
        "[Sized, :(Sized | Flat)]"
    );
    assert_eq!(
        run(&format!("{MAYBE}\n{SHAPE}\nPRINT (Shape.Sized.Some 3)")),
        "Some(3)",
        "it builds no `Sized`"
    );
    assert_eq!(
        run(&format!(
            "{MAYBE}\n{SHAPE}\nLET t = Shape.Sized\nPRINT (ATTR t Some)"
        )),
        "Some",
        "a run-time projection reads through the payload too"
    );
}

#[test]
fn an_annotation_at_the_union_is_settled_at_load() {
    for annotation in [":Maybe", ":(Maybe | Number)"] {
        loaded(
            &format!("{MAYBE}\nLET x {annotation} = (Maybe.Some 3)"),
            |program| {
                let shape = program.shape();
                let annotation = shape
                    .annotation(slot(program, shape, "x"))
                    .expect("`x` is annotated");
                assert!(program.facts().settled(shape, shape.site(annotation)));
            },
        );
    }
}

#[test]
fn a_generic_call_binds_its_name_to_the_union_reached() {
    let what = "EXPR FOR ALL #[Un] #(WHAT x :Un) -> Any = #(PRINT Un)";
    for (call, expected) in [
        ("WHAT (Maybe.Some 3)", ":(Some | None)"),
        (
            "WHAT (Option.Some 3)",
            ":((Some {Elem = Number}) | (None {Elem = Number}))",
        ),
        (
            "WHAT (Tree.Leaf null)",
            ":((Leaf {Elem = Never}) | (Node {Elem = Never}))",
        ),
        ("LET Sm = Maybe.Some\nWHAT (Sm 3)", "Some"),
    ] {
        assert_eq!(
            run(&format!("{MAYBE}\n{OPTION}\n{TREE}\n{what}\n{call}")),
            expected,
            "{call}"
        );
    }
}

#[test]
fn the_lower_end_still_refuses_what_the_variant_never_fits() {
    assert_eq!(
        run(&format!(
            "{MAYBE}\nEXPR #(NONEONLY x :(Maybe.None)) -> Str = #(\"empty\")\n\
             PRINT (NONEONLY (Maybe.Some 1))"
        )),
        "load: <test>:3:7: no overload of `NONEONLY _` admits this argument:\n  at least Some, which \
         can never satisfy None"
    );
    assert_eq!(
        run(&format!(
            "{MAYBE}\nLET f = (FN :{{r :(Maybe.None)}} -> Str = #(\"yes\"))\n\
             PRINT (f {{r = (Maybe.Some 1)}})"
        )),
        "load: <test>:3:7: this argument is at least :{r :Some}, which can never satisfy the \
         parameters :{r :None} of :(FN :{r :None} -> Str)"
    );
    assert_eq!(
        run(&format!("{MAYBE}\nLET x :(Maybe.None) = (Maybe.Some 3)")),
        "load: <test>:2:1: this value is at least Some, which can never satisfy its annotation None"
    );
}
