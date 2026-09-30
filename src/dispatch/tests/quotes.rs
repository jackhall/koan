//! Keyworded uses in a quote's code: what an unmarked use may select, how `USING` fills its key,
//! where `$(…)` and `\(…)` resolve it, and `EVAL` and `USING` as koan expressions.

use super::run;

/// A definition that runs the code it is handed twice.
const TWICE: &str = "EXPR #(TWICE body :Expression) -> Any = #(\n  EVAL body\n  EVAL body\n)\n";

#[test]
fn eval_runs_code_whose_dollar_names_bind_where_the_quote_is_written() {
    assert_eq!(run(&format!("{TWICE}TWICE #(PRINT \"hi\")")), "hi\nhi");
    assert_eq!(
        run(&format!("{TWICE}LET x = \"caller\"\nTWICE #(PRINT $x)")),
        "caller\ncaller"
    );
    assert_eq!(
        run(&format!("{TWICE}LET x = \"caller\"\nTWICE #(PRINT x)")),
        "error: unbound name 'x'"
    );
    assert_eq!(
        run(&format!("{TWICE}TWICE (PRINT \"hi\")")),
        "load: <test>:5:1: no overload of `TWICE _` admits (Str)"
    );
    assert_eq!(
        run(&format!(
            "{TWICE}EXPR #(LOUD x :Str) -> Any = #(PRINT x)\nTWICE (LOUD \"hi\")"
        )),
        "hi\nerror: no overload of TWICE _ admits (Str)",
        "the argument evaluated before the call is selected"
    );
}

#[test]
fn an_unmarked_use_selects_among_builtins_and_what_is_composed_ahead_of_it() {
    assert_eq!(
        run("EXPR #(PRINT x :Number) -> Str = #(\"mine\")\nPRINT (EVAL #(PRINT 1))"),
        "1\n1",
        "an overload visible where the quote is written is no candidate"
    );
    assert_eq!(
        run("PRINT (EVAL #((EXPR #(PRINT x :Number) -> Str = #(\"composed\")) (PRINT 1)))"),
        "composed"
    );
    assert_eq!(
        run("EXPR #(RUN body :Expression) -> Any = #(\n  \
             EXPR #(PRINT x :Number) -> Str = #(\"runner's\")\n  \
             EVAL body\n\
             )\n\
             RUN #(PRINT 1)"),
        "1",
        "nor is one visible where the code runs"
    );
}

#[test]
fn using_fills_a_keyworded_hole_with_a_module_s_registrations_at_its_key() {
    let greet = "MODULE m = (EXPR #(GREET x :Str) -> Str = #(\"hello from m\"))\n";
    assert_eq!(
        run(&format!("{greet}PRINT (EVAL (#(GREET \"bob\") USING m))")),
        "hello from m"
    );
    assert_eq!(
        run(&format!(
            "{greet}LET filled = (#(GREET \"bob\") USING m)\nPRINT (EVAL filled)"
        )),
        "hello from m",
        "`USING` is an expression whose value is the filled code"
    );
    assert_eq!(
        run("PRINT (EVAL #(GREET \"bob\"))"),
        "error: unbound key (GREET _)",
        "a hole some use selects from alone stays required"
    );
}

#[test]
fn a_dollar_group_resolves_its_use_where_the_quote_is_written() {
    assert_eq!(
        run("EXPR #(GREET x :Str) -> Str = #(\"hello\")\n\
             LET q = #($(GREET \"bob\"))\n\
             PRINT (EVAL q)"),
        "hello"
    );
    assert_eq!(
        run("LET q = #($(GREET \"bob\"))"),
        "load: <test>:1:12: `GREET _` has no overload visible here"
    );
}

#[test]
fn a_dollar_group_over_an_operator_run_marks_every_use_the_chain_builds() {
    let words = "{x = \"a\", y = \"b\", z = \"c\"}";
    let less = "OP #(<) OVER Str -> Bool = #(true)\n";
    assert_eq!(
        run(&format!(
            "{less}LET q = #($(x < y < z))\nPRINT (EVAL (q USING {words}))"
        )),
        "true",
        "both `<` uses and the `AND` joining them resolve where the quote is written"
    );
    assert_eq!(
        run(&format!(
            "{less}LET q = #(x < y < z)\nPRINT (EVAL (q USING {words}))"
        )),
        "error: no overload of _ < _ admits (Str, Str)",
        "unmarked, the `<` uses see only the builtin"
    );
}

#[test]
fn a_backslash_group_takes_its_key_from_the_eval_that_runs_it() {
    let run_it = "EXPR #(RUN body :(Expression NEEDING #[(GREET _)])) -> Any = #(\n  \
                  EXPR #(GREET x :Str) -> Str = #(\"offered\")\n  \
                  EVAL body\n\
                  )\n";
    assert_eq!(
        run(&format!("{run_it}PRINT (RUN #(\\(GREET \"bob\")))")),
        "offered"
    );
    assert_eq!(
        run(&format!("{run_it}PRINT (RUN #(\\(WAVE \"bob\")))")),
        "load: <test>:5:7: no overload of `RUN _` admits (:(Expression NEEDING #[(WAVE _)]))",
        "a parameter admits a quote whose needed keys its list covers"
    );
}

#[test]
fn a_named_hole_offered_by_eval_binds_where_the_eval_is_written() {
    let source = "EXPR #(WITH_FIVE body :(Expression NEEDING #[it])) -> Any = #(\n  \
                  LET it = 5\n  \
                  EVAL body\n\
                  )\n\
                  WITH_FIVE #(PRINT \\it)";
    assert_eq!(run(source), "5");
}

#[test]
fn a_using_source_fills_only_the_fields_its_type_names() {
    assert_eq!(
        run("LET r = ({x = 1, y = 2} :! :{x :Number})\n\
             EVAL (#(PRINT y) USING r)"),
        "error: unbound name 'y'"
    );
}
