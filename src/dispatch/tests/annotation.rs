//! `LET <name> <type> = <value>`: a value binder held to the type it states, as `:!` holds its
//! operand — refused at load where the two can never meet, settled where the value always
//! satisfies it, and checked and retyped at run otherwise.

use crate::scope::Site;
use crate::type_lattice::display_name;

use super::run;
use super::statics::{interval, loaded, slot, top, using_block};

#[test]
fn an_annotated_binder_holds_its_value_at_its_type() {
    assert_eq!(run("LET n :Number = 1\nPRINT n"), "1");
    let which = "EXPR #(WHICH x :(LIST OF Number)) -> Str = #(\"numbers\")\n\
                 EXPR #(WHICH x :Any) -> Str = #(\"any\")\n\
                 LET xs :(LIST OF Any) = [1]\n";
    assert_eq!(top(which, "xs"), ":(LIST OF Any)");
    assert_eq!(
        run(&format!("{which}PRINT (WHICH xs)")),
        "any",
        "the value is retyped to its annotation"
    );
}

#[test]
fn an_annotation_its_value_can_never_satisfy_refuses_the_load() {
    assert_eq!(
        run("LET s :Number = \"a\""),
        "load: <test>:1:1: this value is Str, which can never satisfy its annotation Number"
    );
}

#[test]
fn an_annotation_the_load_settles_is_not_checked_at_run() {
    loaded("LET n :Number = 1", |program| {
        let shape = program.shape();
        let annotation = shape
            .annotation(slot(program, shape, "n"))
            .expect("`n` is annotated");
        assert!(shape.settled(Site::of(annotation)));
    });
}

#[test]
fn an_annotation_the_value_misses_at_run_is_an_error_value() {
    assert_eq!(
        run("LET r = ({v = 1} :! :{v :Any})\nLET n :{v :Number} = r\nPRINT \"bound\""),
        "error: :{v :Any} does not satisfy its annotation :{v :Number}"
    );
}

#[test]
fn an_annotated_function_recurses_through_its_own_name() {
    let source = "NEWTYPE Link = :{next :Any}\n\
                  EXPR #(NEXT l :Link WITH f :Any) -> Str = #(f {l = (ATTR l next)})\n\
                  EXPR #(NEXT l :Null WITH f :Any) -> Str = #(\"done\")\n\
                  LET walk :(FN :{l :Any} -> Str) = (FN :{l :Any} -> Str = #(NEXT l WITH walk))\n\
                  PRINT (walk {l = (Link {next = (Link {next = null})})})";
    assert_eq!(run(source), "done");
    assert_eq!(top(source, "walk"), ":(FN :{l :Any} -> Str)");
}

#[test]
fn a_using_over_an_annotated_binder_surfaces_its_annotation_s_names() {
    let source = "SIG Boxes = #[(EXPR FOR ALL #[Elt] #(BOX _ :Elt) -> :(LIST OF Elt))]\n\
                  EXPR #(OPEN m :Any) -> Any = #(\n  \
                  LET held :Boxes = m\n  \
                  USING held SCOPE (LET a = (BOX 1))\n\
                  )";
    using_block(source, |program, block| {
        let typed = interval(program, block, "a");
        let rendered = display_name(typed.upper, program.types(), program.symbols());
        assert_eq!(rendered.to_string(), ":(LIST OF Number)");
    });
}
