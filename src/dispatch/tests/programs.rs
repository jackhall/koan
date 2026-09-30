//! Whole programs over what dispatch evaluates: literals, containers, names, combined definitions,
//! lambdas, type parameters, frames' contracts, record access, construction, blocks, error values,
//! and programs nested to the syntax depth limit, run on the stack the interpreter sizes for it.

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
        run(
            "LET r = ({v = \"s\"} :! :{v :Any})\nEXPR #(BAD) -> Number = #(r.v)\nPRINT (BAD)\nPRINT \"after\""
        ),
        "error: :(FN :{} -> Number) returned Str, which does not satisfy Number"
    );
    assert_eq!(
        run("EXPR #(BAD) -> Number = #(\"s\")\nPRINT (BAD)"),
        "load: <test>:1:26: this body returns Str, which can never satisfy its declared return \
         Number",
        "a body that can never return its declared type refuses the load"
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
                  PRINT (ATTR p (which))";
    assert_eq!(run(source), "a\na\na");
    assert_eq!(
        run("LET p = {x = 1, y = \"a\"}\nPRINT (ATTR p z)"),
        "load: <test>:2:7: :{x :Number y :Str} has no field z",
        "every record `p` can carry lacks `z`"
    );
    assert_eq!(
        run("EXPR #(HIDE x :Any) -> Any = #(x)\n\
             LET p = {x = 1, y = \"a\"}\n\
             PRINT (ATTR (HIDE p) z)"),
        "error: :{x :Number y :Str} has no field z",
        "a record the load cannot read faults at run"
    );
    assert_eq!(
        run("LET p = {y = 1}\nPRINT (ATTR p \"y\")"),
        "load: <test>:2:7: no overload of `ATTR _ _` admits (:{y :Number}, Str)"
    );
    assert_eq!(
        run("NEWTYPE Point = :{x :Number, y :Number}\n\
             LET p = (Point {x = 1, y = 2})\n\
             PRINT p.y"),
        "2",
        "through the newtype over the record"
    );
    assert_eq!(
        run("UNION Maybe = #{Some: Number, None: Null}\nPRINT Maybe.Some"),
        "Some"
    );
    assert_eq!(
        run("UNION Maybe = #{Some: Number, None: Null}\nPRINT Maybe.Many"),
        "load: <test>:2:7: :(Some | None) has no member Many",
        "a closed projection naming no member refuses the load"
    );
    assert_eq!(
        run("MODULE m = (LET x = 1)\nPRINT m.x"),
        "error: reading a module's member arrives with modules"
    );
}

#[test]
fn a_call_by_name_admits_its_arguments_and_solves_its_own_group() {
    assert_eq!(
        run("LET f = (FN :{x :Number} -> Str = #(\"ran\"))\nPRINT (f {x = \"s\"})"),
        "error: :(FN :{x :Number} -> Str) cannot be called with :{x :Str}"
    );
    let pair = "LET f = (FN FOR ALL #[Elt] :{x :Elt, y :Elt} -> Str = #((PRINT Elt) (\"ran\")))\n";
    assert_eq!(
        run(&format!("{pair}PRINT (f {{x = 1, y = 2}})")),
        "Number\nran"
    );
    assert_eq!(
        run(&format!("{pair}PRINT (f {{x = 1, y = 2, Elt = Str}})")),
        "error: arguments :{x :Number y :Number Elt :ProperType} do not name the parameters of \
         :(FN FOR ALL #[Elt] :{x :Elt y :Elt} -> Str)",
        "a type parameter is solved, never written"
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
                  PRINT (PICK (#[x y] FROM both))";
    assert_eq!(run(source), "{x = 1, y = a}\ngot xy");
    assert_eq!(
        run("LET both = {x = 1, y = \"a\", z = \"b\"}\nPRINT (#[x q] FROM both)"),
        "load: <test>:2:7: :{x :Number y :Str z :Str} has no field q",
        "every record `both` can carry lacks `q`"
    );
    assert_eq!(
        run("EXPR #(HIDE x :Any) -> Any = #(x)\n\
             LET both = {x = 1, y = \"a\", z = \"b\"}\n\
             PRINT (#[x q] FROM (HIDE both))"),
        "error: :{x :Number y :Str z :Str} has no field q",
        "a record the load cannot read faults at run"
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
fn eval_over_a_number_refuses_the_load() {
    assert_eq!(
        run("LET n = 1\nPRINT (EVAL n -> Any)"),
        "load: <test>:2:7: this value is Number, which can never be code for `EVAL` to run"
    );
}

#[test]
fn an_uncaught_error_ends_the_program_with_its_message() {
    assert_eq!(
        run(
            "LET r = ({v = \"a\"} :! :{v :Any})\nPRINT \"before\"\nPRINT (1 + r.v)\nPRINT \"after\""
        ),
        "before\nerror: no overload of _ + _ admits (Number, Str)"
    );
}

#[test]
fn every_evaluation_passes_an_error_it_receives_through_unchanged() {
    assert_eq!(
        run("LET r = ({v = \"s\"} :! :{v :Any})\n\
             EXPR #(ONE x :Number) -> Number = #(x)\n\
             PRINT [1, (ONE (ONE r.v))]"),
        "error: no overload of ONE _ admits (Str)"
    );
    assert_eq!(
        run(
            "LET r = ({v = \"a\"} :! :{v :Any})\nEXPR #(DEEP) -> Number = #(1 + r.v)\nPRINT {n = (DEEP)}"
        ),
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

/// A program at `depth` in each nesting shape, beside what it prints: `depth` is the stored depth
/// of its deepest statement, and each shape's count of groups or operators is read off how the
/// parse counts it.
fn nested_programs(depth: usize) -> Vec<(&'static str, String, String)> {
    let parens = |n: usize| format!("{}1{}", "(".repeat(n), ")".repeat(n));
    let records = |n: usize| format!("{}1{}", "{a = ".repeat(n), "}".repeat(n));
    // One level for `PRINT` or `LET`, the rest for what it holds.
    let inner = depth - 1;
    // A run of `k` operators counts `k + 3` above its deepest operand.
    let folded = inner - 3;
    let pairwise = inner - 3 - 1;
    let operands = (0..=pairwise)
        .map(|index| format!("(0 + {})", 1 + index % 2))
        .collect::<Vec<_>>()
        .join(" != ");
    vec![
        (
            "parentheses",
            format!("PRINT {}", parens(inner)),
            "1".into(),
        ),
        (
            "a quote",
            format!("LET q = #{}\nPRINT (q == q)\nPRINT q", parens(inner)),
            // A quote prints its code through the parentheses written around it.
            "true\n1".into(),
        ),
        (
            "a list literal",
            format!("PRINT {}1{}", "[".repeat(inner), "]".repeat(inner)),
            format!("{}1{}", "[".repeat(inner), "]".repeat(inner)),
        ),
        (
            "a record literal",
            format!("PRINT {}", records(inner)),
            records(inner),
        ),
        (
            "a dotted chain",
            format!("LET r = {}\nPRINT r{}", records(inner), ".a".repeat(inner)),
            "1".into(),
        ),
        (
            "a folded operator run",
            format!("PRINT (1{})", " + 1".repeat(folded)),
            (folded + 1).to_string(),
        ),
        (
            "a pairwise operator run",
            format!("PRINT ({operands})"),
            "true".into(),
        ),
    ]
}

#[test]
fn a_program_nested_to_the_limit_runs_and_one_level_more_is_refused() {
    use crate::memory::program_storage;
    use crate::parse::{MAX_SYNTAX_DEPTH, parse};
    use crate::program::STACK_BYTES;
    use crate::symbols::SymbolInterner;

    let deepest = |source: &str| {
        let program = program_storage();
        let symbols = SymbolInterner::new();
        let statements = parse(program.brand(), &symbols, source).expect("parses");
        statements.iter().map(|s| s.depth()).max()
    };
    let check = move || {
        for (shape, source, printed) in nested_programs(MAX_SYNTAX_DEPTH) {
            assert_eq!(deepest(&source), Some(MAX_SYNTAX_DEPTH), "{shape}");
            assert_eq!(run(&source), printed, "{shape} at the limit");
        }
        for (shape, source, _) in nested_programs(MAX_SYNTAX_DEPTH + 1) {
            let refused = run(&source);
            assert!(
                refused.starts_with("load: ") && refused.contains(&MAX_SYNTAX_DEPTH.to_string()),
                "{shape} past the limit: {refused}"
            );
        }
    };
    std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(check)
        .expect("spawn")
        .join()
        .expect("runs within the stack");
}

#[test]
fn a_type_expression_runs_as_its_load_time_type() {
    assert_eq!(run("PRINT :(LIST OF Number)"), ":(LIST OF Number)");
    let source = "EXPR FOR ALL #[Elt] #(TWIN x :Elt AND y :Elt) -> Any = #(:(LIST OF Elt))\n\
                  PRINT (TWIN 1 AND 2)\n\
                  PRINT (TWIN \"a\" AND \"b\")";
    assert_eq!(
        run(source),
        ":(LIST OF Number)\n:(LIST OF Str)",
        "a rigid type takes each call's solution"
    );
}

#[test]
fn a_type_parameter_binds_the_argument_its_call_passes() {
    let body = "-> Any = #((PRINT Elt) (:(LIST OF Elt)))";
    assert_eq!(
        run(&format!(
            "EXPR #(MAKESET Elt :Type) {body}\nPRINT (MAKESET Number)"
        )),
        "Number\n:(LIST OF Number)",
        "by keyword"
    );
    assert_eq!(
        run(&format!(
            "LET make = FN EXPR #(MAKESET Elt :Type) {body}\nPRINT (make {{Elt = Str}})"
        )),
        "Str\n:(LIST OF Str)",
        "by name"
    );
}

#[test]
fn a_closed_type_that_does_not_elaborate_refuses_the_load() {
    assert_eq!(
        run("NEWTYPE Bad = :(Number.z)"),
        "load: <test>:1:15: Number has no member z"
    );
    assert_eq!(
        run("LET f = (FN :{} -> Any = #(:(Number.z)))"),
        "load: <test>:1:28: Number has no member z",
        "in the body of a callable no one calls"
    );
    assert_eq!(
        run("LET q = #(PRINT :(Number.z))\nPRINT \"loaded\"\nEVAL q -> Any"),
        "loaded\nerror: <test>:1:17: Number has no member z",
        "in a quote's code, reported by the `EVAL` that runs it"
    );
}

#[test]
fn the_overlap_check_reads_declared_types() {
    assert_eq!(
        run("LET Num = Number\nOP #(+) OVER Num = #(0)"),
        "load: <test>:2:1: this overload of `_ + _` takes operands the builtin \
         :(EXPR #(_ :Number + _ :Number) -> Number) already takes"
    );
}

#[test]
fn a_callable_over_a_run_bound_type_is_born_with_the_solution() {
    let source = "LET mk = (FN FOR ALL #[Elt] :{x :Elt} -> :(FN :{y :Elt} -> Elt) = \
                  #(FN :{y :Elt} -> Elt = #(y)))\n\
                  LET g = (mk {x = 1})\n\
                  PRINT g\n\
                  PRINT (g {y = 2})\n\
                  PRINT (g {y = \"s\"})";
    assert_eq!(
        run(source),
        ":(FN :{y :Number} -> Number)\n2\n\
         error: :(FN :{y :Number} -> Number) cannot be called with :{y :Str}"
    );
}

#[test]
fn a_captured_type_parameter_reads_the_enclosing_call_s_solution() {
    let source = "LET mk = (FN FOR ALL #[Elt] :{x :Elt, y :Elt} -> :(FN :{} -> Any) = \
                  #(FN :{} -> Any = #(:(LIST OF Elt))))\n\
                  PRINT ((mk {x = 1, y = 2}) {})\n\
                  PRINT ((mk {x = \"s\", y = \"t\"}) {})";
    assert_eq!(run(source), ":(LIST OF Number)\n:(LIST OF Str)");
}

#[test]
fn a_nominal_over_a_run_bound_type_is_declared_per_call() {
    let source = "LET mk = (FN FOR ALL #[Elt] :{x :Elt, y :Elt} -> Any = \
                  #((NEWTYPE Boxed = :{v :Elt}) (Boxed {v = x})))\n\
                  PRINT (mk {x = 1, y = 2})\n\
                  PRINT (mk {x = \"a\", y = \"b\"})";
    assert_eq!(run(source), "Boxed({v = 1})\nBoxed({v = a})");
}
