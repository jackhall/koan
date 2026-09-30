//! Which candidate a keyworded call runs: admission by carried type, the class-by-class order,
//! ambiguity, the no-overload miss, and how a user overload meets a builtin's.

use super::run;

#[test]
fn a_candidate_list_keys_on_the_full_bucket_key() {
    assert_eq!(
        run("EXPR #(MOVE x :Number TO y :Number) -> Str = #(\"two\")\nPRINT (MOVE 1)"),
        "load: <test>:2:7: `MOVE _` has no overload visible here"
    );
}

#[test]
fn a_typed_argument_a_candidate_does_not_admit_falls_through_to_the_next() {
    let source = "EXPR #(PICK x :Str) -> Str = #(\"str\")\n\
                  EXPR #(PICK x :Any) -> Str = #(\"any\")\n\
                  PRINT (PICK 1)\n\
                  PRINT (PICK \"s\")";
    assert_eq!(run(source), "any\nstr");
}

#[test]
fn the_first_class_decides_when_it_orders_the_candidates() {
    let source = "EXPR #(SHOW x :Number WITH y :Any) -> Str = #(\"number first\")\n\
                  EXPR FOR ALL #[Elt] #(SHOW x :Elt WITH y :Str) -> Str = #(\"str second\")\n\
                  PRINT (SHOW 1 WITH \"s\")";
    assert_eq!(run(source), "number first");
}

#[test]
fn a_class_that_orders_neither_candidate_passes_both_to_the_next() {
    let source = "EXPR #(MOVE x :(Str | Bool) TO y :Number) -> Str = #(\"narrow\")\n\
                  EXPR #(MOVE x :(Number | Str) TO y :Any) -> Str = #(\"wide\")\n\
                  PRINT (MOVE \"s\" TO 1)";
    assert_eq!(run(source), "narrow");
}

#[test]
fn a_keyworded_call_solves_a_group_class_by_class_and_a_call_by_name_jointly() {
    let pair = "EXPR FOR ALL #[Elt] #(PAIR x :(LIST OF Elt) WITH y :(LIST OF Elt)) -> Str = \
                #(\"paired\")\n";
    let mixed = "EXPR #(MIXED) -> Any = #([1, \"x\"])\n";
    assert_eq!(
        run(&format!(
            "{pair}{mixed}PRINT (PAIR [1, \"x\"] WITH [1])\nPRINT (PAIR [1] WITH (MIXED))"
        )),
        "paired\nerror: no overload of PAIR _ WITH _ admits \
         (:(LIST OF Number), :(LIST OF :(Number | Str)))",
        "the first class fixes `Elt`, and the second must lie under it"
    );
    assert_eq!(
        run(&format!("{pair}PRINT (PAIR [1] WITH [1, \"x\"])")),
        "load: <test>:2:7: no overload of `PAIR _ WITH _` admits \
         (:(LIST OF Number), :(LIST OF :(Number | Str)))",
        "an exact second argument above `Elt`'s solution refuses the load"
    );
    assert_eq!(
        run(&format!(
            "EXPR #(PAIR 1 WITH 1)\n{pair}\
             PRINT (PAIR [1, \"x\"] WITH [1])\nPRINT (PAIR [1] WITH [1, \"x\"])"
        )),
        "paired\npaired",
        "one class solves both slots jointly"
    );
    assert_eq!(
        run(&format!(
            "LET pair = FN {pair}PRINT (pair {{x = [1], y = [1, \"x\"]}})"
        )),
        "paired",
        "a call by name solves the group jointly"
    );
}

#[test]
fn one_variable_over_both_slots_ranks_before_two_independent_ones() {
    let source = "EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str = #(\"joint\")\n\
                  EXPR FOR ALL #[First Second] #(PAIR x :First WITH y :Second) -> Str = \
                  #(\"apart\")\n\
                  PRINT (PAIR 1 WITH 2)";
    assert_eq!(run(source), "joint");
}

#[test]
fn one_variable_in_two_classes_takes_its_type_from_the_first() {
    let source = "EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str = #(\"p\")\n\
                  PRINT (PAIR 1 WITH \"x\")";
    assert_eq!(
        run(source),
        "load: <test>:2:7: no overload of `PAIR _ WITH _` admits (Number, Str)"
    );
}

#[test]
fn a_variable_a_class_did_not_admit_reads_as_its_bound_later() {
    let source = "EXPR #(TAKE x :(Str | (LIST OF Number)) WITH y :Number) -> Str = #(\"mono\")\n\
                  EXPR FOR ALL #[Elt] #(TAKE x :(Number | (LIST OF Elt)) WITH y :Elt) -> Str = \
                  #(\"poly\")\n\
                  PRINT (TAKE [1] WITH 1)";
    assert_eq!(run(source), "mono");
}

#[test]
fn two_admitting_candidates_neither_ranks_first_are_ambiguous_wherever_declared() {
    let expected = "error: ambiguous call of PICK _: 2 overloads admit (Number) and none ranks \
                    first";
    assert_eq!(
        run("EXPR #(PICK x :Number) -> Str = #(\"a\")\n\
             EXPR #(PICK x :Number) -> Str = #(\"b\")\n\
             LET r = ({v = 1} :! :{v :Any})\n\
             PRINT (PICK r.v)"),
        expected,
        "one scope"
    );
    assert_eq!(
        run("EXPR #(PICK x :Number) -> Str = #(\"outer\")\n\
             LET r = ({v = 1} :! :{v :Any})\n\
             EXPR #(INNER) -> Str = #(\n  \
             EXPR #(PICK x :Number) -> Str = #(\"inner\")\n  \
             PICK r.v\n\
             )\n\
             PRINT (INNER)"),
        expected,
        "no scope shadows another's overload"
    );
}

#[test]
fn a_certain_ambiguity_refuses_the_load() {
    assert_eq!(
        run("EXPR #(PICK x :Number) -> Str = #(\"a\")\n\
             EXPR #(PICK x :Number) -> Str = #(\"b\")\n\
             PRINT (PICK 1)"),
        "load: <test>:3:7: ambiguous call of PICK _: 2 overloads admit (Number) and none ranks \
         first",
        "one scope"
    );
    assert_eq!(
        run("EXPR #(PICK x :Number) -> Str = #(\"outer\")\n\
             EXPR #(INNER) -> Str = #(\n  \
             EXPR #(PICK x :Number) -> Str = #(\"inner\")\n  \
             PICK 1\n\
             )\n\
             PRINT (INNER)"),
        "load: <test>:4:3: ambiguous call of PICK _: 2 overloads admit (Number) and none ranks \
         first",
        "no scope shadows another's overload"
    );
}

#[test]
fn no_admitting_candidate_is_a_miss_naming_the_argument_types() {
    assert_eq!(
        run("LET r = ({v = \"a\"} :! :{v :Any})\nPRINT (r.v + 1)"),
        "error: no overload of _ + _ admits (Str, Number)"
    );
    assert_eq!(
        run("PRINT (\"a\" + 1)"),
        "load: <test>:1:7: no overload of `_ + _` admits (Str, Number)",
        "a use no candidate can admit refuses the load"
    );
    assert_eq!(
        run("EXPR #(PICK x :Str) -> Str = #(\"a\")\nPRINT (PICK {n = 1})"),
        "load: <test>:2:7: no overload of `PICK _` admits (:{n :Number})"
    );
}

#[test]
fn a_builtin_wins_a_tie_with_a_user_overload() {
    assert_eq!(
        run("OP #(==) OVER Any -> Bool = #(false)\nPRINT (1 == 1)"),
        "true"
    );
    assert_eq!(
        run("EXPR #(PRINT x :Any) -> Str = #(\"mine\")\nPRINT 1"),
        "1"
    );
}

#[test]
fn a_user_equality_is_selected_in_the_builtin_s_place_for_its_type() {
    let source = "NEWTYPE Point = :{x :Number, y :Number}\n\
                  OP #(==) OVER Point -> Bool = #(true)\n\
                  PRINT ((Point {x = 1, y = 2}) == (Point {x = 3, y = 4}))\n\
                  PRINT (1 == 2)\n\
                  PRINT ((Point {x = 1, y = 2}) != (Point {x = 3, y = 4}))";
    assert_eq!(run(source), "true\nfalse\nfalse");
}

#[test]
fn a_user_print_is_selected_for_its_type() {
    let source = "EXPR #(PRINT x :Number) -> Str = #(\"mine\")\n\
                  PRINT 1\n\
                  PRINT \"s\"\n\
                  PRINT (PRINT 2)";
    assert_eq!(run(source), "s\nmine");
}

#[test]
fn a_user_operator_is_admitted_beside_a_builtin_it_does_not_overlap() {
    assert_eq!(
        run("OP #(+) OVER Str = #(\"joined\")\nPRINT (\"a\" + \"b\")\nPRINT (1 + 2)"),
        "joined\n3"
    );
    assert_eq!(
        run("OP #(+) OVER Number = #(0)\nPRINT (1 + 2)"),
        "load: <test>:1:1: this overload of `_ + _` takes operands the builtin \
         :(EXPR #(_ :Number + _ :Number) -> Number) already takes"
    );
    assert_eq!(
        run("EXPR #(ADD) -> Any = #(\n  OP #(+) OVER Number = #(0)\n  1 + 2\n)"),
        "load: <test>:2:3: this overload of `_ + _` takes operands the builtin \
         :(EXPR #(_ :Number + _ :Number) -> Number) already takes",
        "a body nested in the program is checked with it"
    );
}

#[test]
fn code_an_eval_runs_is_checked_for_overlaps_where_it_runs() {
    let source = "LET q = #((OP #(+) OVER Number = #(0)) (1 + 2))\n\
                  PRINT \"loaded\"\n\
                  EVAL q -> Any";
    assert_eq!(
        run(source),
        "loaded\nerror: <test>:1:11: this overload of `_ + _` takes operands the builtin \
         :(EXPR #(_ :Number + _ :Number) -> Number) already takes"
    );
}

#[test]
fn a_candidate_that_may_admit_still_solves_its_group_from_the_carried_types() {
    let source = "EXPR #(EITHER) -> (Number | Str) = #(1)\n\
                  EXPR FOR ALL #[Elt] #(BOTH x :Elt AND y :Elt) -> Str = #(\"both\")\n\
                  PRINT (BOTH (EITHER) AND \"s\")";
    assert_eq!(
        run(source),
        "error: no overload of BOTH _ AND _ admits (Number, Str)"
    );
}
