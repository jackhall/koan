//! Whole programs over what dispatch evaluates: literals, containers, names, combined definitions,
//! lambdas, type parameters, frames' contracts, record access, construction, blocks and error
//! values.

use super::run;

#[test]
fn the_builtin_library_runs_over_literals_containers_and_names() {
    let source = "LET x = 4\n\
                  PRINT (x - 1)\n\
                  PRINT (6 / 4 * 2)\n\
                  PRINT (1 < 2 < 3)\n\
                  PRINT ((1 < 2) AND (NOT (2 < 1)))\n\
                  PRINT ([1, 2, 3] == [1, 2, 3])\n\
                  PRINT (1 != 2)\n\
                  PRINT [x, \"a\"]\n\
                  PRINT {\"width\": x}\n\
                  PRINT {n = x, s = \"a\"}\n\
                  PRINT (Number | Str)\n\
                  PRINT (Number & Any)";
    assert_eq!(
        run(source),
        "3\n3\ntrue\ntrue\ntrue\ntrue\n[4, a]\n{\"width\": 4}\n{n = 4, s = a}\n\
         :(Number | Str)\nNumber"
    );
}

#[test]
fn print_returns_the_string_it_printed() {
    assert_eq!(run("PRINT (PRINT 1)"), "1\n1");
}

#[test]
fn a_pairwise_run_hoists_an_operand_into_a_block_it_evaluates_once() {
    let source = "EXPR #(NOISY x :Number) -> Number = #((PRINT \"once\") (x))\n\
                  PRINT (1 < (NOISY 2) < 3)";
    assert_eq!(run(source), "once\ntrue");
}

#[test]
fn a_combined_definition_binds_its_name_and_registers_its_shape() {
    assert_eq!(
        run(
            "LET double = FN EXPR #(DOUBLE x :Number) -> Number = #(x * 2)\n\
             PRINT (DOUBLE 2)\n\
             PRINT (double {x = 3})"
        ),
        "4\n6"
    );
    assert_eq!(
        run("LET plus = OP #(+) OVER Str = #(\"joined\")\n\
             PRINT (\"a\" + \"b\")\n\
             PRINT (plus {left = \"a\", right = \"b\"})"),
        "joined\njoined"
    );
}

#[test]
fn a_function_no_binder_names_is_born_where_it_is_evaluated() {
    let source = "EXPR #(CONSTANTLY value :Str) -> :(FN :{} -> Str) = #(\n  \
                  FN :{} -> Str = #(value)\n\
                  )\n\
                  LET always_hi = (CONSTANTLY \"hi\")\n\
                  PRINT (always_hi {})";
    assert_eq!(run(source), "hi");
}

#[test]
fn a_keyworded_call_binds_each_type_parameter_to_its_solution() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> :(LIST OF Elt) = #((PRINT Elt) ([x]))\n\
                  PRINT (WRAP 1)\n\
                  PRINT (WRAP \"s\")";
    assert_eq!(run(source), "Number\n[1]\nStr\n[s]");
}

#[test]
fn a_frame_s_value_carries_its_declared_return() {
    let source = "EXPR #(NUMS) -> :(LIST OF Any) = #([1, 2])\n\
                  EXPR #(WHICH x :(LIST OF Number)) -> Str = #(\"numbers\")\n\
                  EXPR #(WHICH x :(LIST OF Any)) -> Str = #(\"any\")\n\
                  PRINT (WHICH (NUMS))\n\
                  PRINT (WHICH [1])";
    assert_eq!(run(source), "any\nnumbers", "a container is retyped");
    let source = "UNION Maybe = #{Some: Number, None: Null}\n\
                  EXPR #(WRAP x :Number) -> Maybe = #(Maybe.Some x)\n\
                  EXPR #(KIND x :Maybe) -> Str = #(\"maybe\")\n\
                  EXPR #(KIND x :(Maybe.Some)) -> Str = #(\"some\")\n\
                  PRINT (KIND (WRAP 1))";
    assert_eq!(
        run(source),
        "some",
        "a tagged value takes the member naming its constructor"
    );
}

#[test]
fn a_return_that_misses_its_declared_type_is_an_error_value() {
    assert_eq!(
        run("EXPR #(BAD) -> Number = #(\"s\")\nPRINT (BAD)\nPRINT \"after\""),
        "error: :(FN :{} -> Number) returned Str, which does not satisfy Number"
    );
}

#[test]
fn a_lone_keyword_is_a_call_of_its_key() {
    assert_eq!(run("EXPR #(ONE) -> Number = #(1)\nONE\nPRINT (ONE)"), "1");
    assert_eq!(
        run("PRINT (NOPE)"),
        "load: <test>:1:7: `NOPE` has no overload visible here"
    );
}

#[test]
fn attr_reads_a_field_by_a_label_written_bare_or_quoted() {
    let source = "LET p = {x = 1, y = \"a\"}\n\
                  PRINT (ATTR p y)\n\
                  PRINT p.y\n\
                  LET which = #(y)\n\
                  PRINT (ATTR p (which))\n\
                  PRINT (ATTR p z)";
    assert_eq!(
        run(source),
        "a\na\na\nerror: :{x :Number y :Str} has no field z"
    );
    assert_eq!(
        run("LET p = {y = 1}\nPRINT (ATTR p \"y\")"),
        "error: no overload of ATTR _ _ admits (:{y :Number}, Str)"
    );
    assert_eq!(
        run("NEWTYPE Point = :{x :Number, y :Number}\n\
             LET p = (Point {x = 1, y = 2})\n\
             PRINT p.y"),
        "2",
        "through the newtype over the record"
    );
    assert_eq!(
        run("UNION Maybe = #{Some: Number, None: Null}\nPRINT Maybe.Some\nPRINT Maybe.Many"),
        "Some\nerror: :(Some | None) has no member Many"
    );
    assert_eq!(
        run("MODULE m = (LET x = 1)\nPRINT m.x"),
        "error: reading a module's member arrives with modules"
    );
}

#[test]
fn a_type_s_field_is_the_type_its_record_declares() {
    let source = "NEWTYPE Point = :{x :Number, y :Str}\n\
                  NEWTYPE Boxed = Point\n\
                  PRINT Point.y\n\
                  PRINT Boxed.x\n\
                  LET which = #(y)\n\
                  PRINT (ATTR Point (which))\n\
                  PRINT Point.z";
    assert_eq!(run(source), "Str\nNumber\nStr\nerror: Point has no field z");
    assert_eq!(
        run("PRINT Number.y"),
        "error: Number has no field y",
        "a type with no record under it"
    );
}

#[test]
fn from_projects_a_record_to_the_fields_it_names() {
    let source = "EXPR #(PICK r :{x :Number, y :Str}) -> Str = #(\"got xy\")\n\
                  EXPR #(PICK r :{x :Number, z :Str}) -> Str = #(\"got xz\")\n\
                  LET both = {x = 1, y = \"a\", z = \"b\"}\n\
                  PRINT (#[x y] FROM both)\n\
                  PRINT (PICK (#[x y] FROM both))\n\
                  PRINT (#[x q] FROM both)";
    assert_eq!(
        run(source),
        "{x = 1, y = a}\ngot xy\nerror: :{x :Number y :Str z :Str} has no field q"
    );
}

#[test]
fn a_type_headed_application_is_a_construction() {
    let source = "UNION Maybe = #{Some: Number, None: Null}\n\
                  NEWTYPE Meters = Number\n\
                  PRINT (Maybe.Some 42)\n\
                  PRINT (Meters 3)\n\
                  PRINT (Maybe.Some \"s\")";
    assert_eq!(
        run(source),
        "Some(42)\nMeters(3)\nerror: Some cannot wrap Str: its representation is Number"
    );
}

#[test]
fn eval_over_a_number_is_a_no_overload_miss() {
    assert_eq!(
        run("LET n = 1\nPRINT (EVAL n)"),
        "error: no overload of EVAL _ admits (Number)"
    );
}

#[test]
fn an_uncaught_error_ends_the_program_with_its_message() {
    assert_eq!(
        run("PRINT \"before\"\nPRINT (1 + \"a\")\nPRINT \"after\""),
        "before\nerror: no overload of _ + _ admits (Number, Str)"
    );
}

#[test]
fn every_evaluation_passes_an_error_it_receives_through_unchanged() {
    assert_eq!(
        run("EXPR #(ONE x :Number) -> Number = #(x)\nPRINT [1, (ONE (ONE \"s\"))]"),
        "error: no overload of ONE _ admits (Str)"
    );
    assert_eq!(
        run("EXPR #(DEEP) -> Number = #(1 + \"a\")\nPRINT {n = (DEEP)}"),
        "error: no overload of _ + _ admits (Number, Str)",
        "through a frame's contract"
    );
}

#[test]
fn a_dict_key_that_is_no_scalar_is_an_error_value() {
    assert_eq!(
        run("LET k = [1]\nPRINT {k: 1}"),
        "error: :(LIST OF Number) cannot be a dict key"
    );
}
