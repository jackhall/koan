//! Keyworded uses in a quote's code: what an unmarked use may select, how `USING` fills its key,
//! where `$(…)` and `\(…)` resolve it, and `EVAL` and `USING` as koan expressions.

use super::run;

/// A definition that runs the code it is handed twice.
const TWICE: &str =
    "EXPR #(TWICE body :Expression) -> Any = #(\n  EVAL body -> Any\n  EVAL body -> Any\n)\n";

#[test]
fn eval_runs_code_to_its_declared_return() {
    assert_eq!(run("PRINT (EVAL #(1 + 2) -> Number)"), "3");
    assert_eq!(
        run("LET q = #([1])\nPRINT (EVAL q -> (LIST OF Number))"),
        "[1]",
        "the return may be spelled bare, as a `FN`'s may"
    );
    assert_eq!(
        run("LET q = #(PRINT 1)\nEVAL q"),
        "load: <test>:2:1: `EVAL _` has no overload visible here",
        "an `EVAL` that declares nothing is no builtin shape"
    );
    assert_eq!(
        run("EXPR #(EVAL c :Code) -> Str = #(\"mine\")\nPRINT (EVAL #(1))"),
        "mine",
        "a program may register at `EVAL _`"
    );
    assert_eq!(
        run("LET q = #(EVAL #(1))\nPRINT \"loaded\"\nEVAL q -> Any"),
        "loaded\nerror: unbound key (EVAL _)",
        "in a quote's code it is a hole, which the `EVAL` running the code reports"
    );
}

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
        run("EXPR #(PRINT x :Number) -> Str = #(\"mine\")\nPRINT (EVAL #(PRINT 1) -> Any)"),
        "1\n1",
        "an overload visible where the quote is written is no candidate"
    );
    assert_eq!(
        run("PRINT (EVAL #((EXPR #(PRINT x :Number) -> Str = #(\"composed\")) (PRINT 1)) -> Any)"),
        "composed"
    );
    assert_eq!(
        run("EXPR #(RUN body :Expression) -> Any = #(\n  \
             EXPR #(PRINT x :Number) -> Str = #(\"runner's\")\n  \
             EVAL body -> Any\n\
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
        run(&format!(
            "{greet}PRINT (EVAL (#(GREET \"bob\") USING m) -> Any)"
        )),
        "hello from m"
    );
    assert_eq!(
        run(&format!(
            "{greet}LET filled = (#(GREET \"bob\") USING m)\nPRINT (EVAL filled -> Any)"
        )),
        "hello from m",
        "`USING` is an expression whose value is the filled code"
    );
    assert_eq!(
        run("PRINT (EVAL #(GREET \"bob\") -> Any)"),
        "error: unbound key (GREET _)",
        "a hole some use selects from alone stays required"
    );
}

/// A key's candidates are laid down typed `List<Any>`, so a quantified registration stands at a
/// hole a `USING` fills, and at a key an `EVAL` offers, beside any other.
#[test]
fn a_quantified_registration_fills_a_hole_and_is_offered_at_its_key() {
    let boxed = "EXPR FOR ALL #[Elt] #(BOX x :Elt) -> :(LIST OF Elt) = #([x])";
    assert_eq!(
        run(&format!(
            "MODULE m = ({boxed})
PRINT (EVAL (#(BOX 1) USING m) -> Any)"
        )),
        "[1]"
    );
    assert_eq!(
        run(&format!(
            "EXPR #(RUN body :(Expression NEEDING #[(BOX _)])) -> Any = #(\n  \
             {boxed}\n  \
             EVAL body -> Any\n\
             )\n\
             PRINT (RUN #(\\(BOX \"a\")))"
        )),
        "[a]"
    );
}

#[test]
fn a_dollar_group_resolves_its_use_where_the_quote_is_written() {
    assert_eq!(
        run("EXPR #(GREET x :Str) -> Str = #(\"hello\")\n\
             LET q = #($(GREET \"bob\"))\n\
             PRINT (EVAL q -> Any)"),
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
            "{less}LET q = #($(x < y < z))\nPRINT (EVAL (q USING {words}) -> Any)"
        )),
        "true",
        "both `<` uses and the `AND` joining them resolve where the quote is written"
    );
    assert_eq!(
        run(&format!(
            "{less}LET q = #(x < y < z)\nPRINT (EVAL (q USING {words}) -> Any)"
        )),
        "error: no overload of _ < _ admits (Str, Str)",
        "unmarked, the `<` uses see only the builtin"
    );
}

#[test]
fn a_backslash_group_takes_its_key_from_the_eval_that_runs_it() {
    let run_it = "EXPR #(RUN body :(Expression NEEDING #[(GREET _)])) -> Any = #(\n  \
                  EXPR #(GREET x :Str) -> Str = #(\"offered\")\n  \
                  EVAL body -> Any\n\
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
                  EVAL body -> Any\n\
                  )\n\
                  WITH_FIVE #(PRINT \\it)";
    assert_eq!(run(source), "5");
}

#[test]
fn a_using_source_fills_only_the_fields_its_type_names() {
    assert_eq!(
        run("LET r = ({x = 1, y = 2} :! :{x :Number})\n\
             EVAL (#(PRINT y) USING r) -> Any"),
        "error: unbound name 'y'"
    );
}

#[test]
fn a_refused_code_hands_nothing_to_the_next_body() {
    assert_eq!(
        run(concat!(
            "LET q = #((SIG Shown = #[(EXPR #(SHOW _ :Number) -> Str)]) ",
            "(USING (m :! Shown) SCOPE (1 + 2 == 3)))\n",
            "LET f = (FN :{} -> Number = #(1))\n",
            "PRINT (f {})",
        )),
        "1"
    );
}

#[test]
fn a_built_use_selects_among_the_codes_registrations_and_the_offered_ones() {
    let source = concat!(
        "EXPR #(RUN body :(Block NEEDING #[(GREET _)])) -> Any = #(\n",
        "  EXPR #(GREET x :Str) -> Str = #(\"offered\")\n",
        "  EVAL body -> Any\n",
        ")\n",
        "PRINT (RUN #((EXPR #(GREET x :Number) -> Str = #(\"own\")) (\\(GREET 1))))\n",
        "PRINT (RUN #((EXPR #(GREET x :Number) -> Str = #(\"own\")) (\\(GREET \"bob\"))))",
    );
    assert_eq!(run(source), "own\noffered");
}
