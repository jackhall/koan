//! Named code as a callable's body: a name the load traces to a written quote, whose body shape
//! is built at load from the quote's code — its holes and `\` marks filled from the callable's
//! parameters, its `\` marks then from the declaring scope, and its `$` names where the quote is
//! written — and code the load cannot trace, built at the callable's first call.

use super::run;

/// `SIG Collection`, which a module built by a call in a traced body is viewed at.
const COLLECTION: &str = "SIG Collection FOR ALL #[Set] = #[(VAL empty :Set)]\n";

#[test]
fn a_traced_name_is_the_body_of_a_function_and_an_expression() {
    assert_eq!(
        run("LET scaled = #(x * \\factor)
LET factor = 3
LET triple = (FN :{x :Number} -> Number = scaled)
EXPR #(TRIPLE x :Number) -> Number = scaled
PRINT (triple {x = 2})
PRINT (TRIPLE 2)"),
        "6\n6"
    );
}

#[test]
fn holes_and_marks_both_fill_from_parameters() {
    assert_eq!(
        run("LET area = #(w * \\h)
EXPR #(AREA w :Number h :Number) -> Number = area
PRINT (AREA 2 3)"),
        "6"
    );
}

#[test]
fn a_hole_no_parameter_fills_is_unbound_at_load() {
    assert_eq!(
        run("LET k = 1
LET body = #(w * k)
EXPR #(AREA w :Number h :Number) -> Number = body"),
        "load: <test>:2:18: `k` names no binding visible here"
    );
}

#[test]
fn the_declaring_scope_fills_a_mark() {
    assert_eq!(
        run("LET k = 10
LET body = #(w + \\k)
EXPR #(ADDK w :Number) -> Number = body
PRINT (ADDK 1)"),
        "11"
    );
}

#[test]
fn a_dollar_name_resolves_where_the_quote_is_written() {
    assert_eq!(
        run("LET k = 2
LET body = #(\\x * $k)
EXPR #(MAKE) -> Any = #(
  LET k = 100
  FN :{x :Number} -> Number = body
)
PRINT ((MAKE) {x = 3})"),
        "6"
    );
}

#[test]
fn a_mistyped_dollar_use_is_refused_at_load() {
    assert_eq!(
        run("LET s = \"a\"
LET bad = #($s * \\x)
EXPR #(BAD x :Number) -> Number = bad"),
        "load: <test>:2:12: no overload of `_ * _` admits (Str, Number)"
    );
}

#[test]
fn the_declared_return_is_checked_at_load() {
    assert_eq!(
        run("LET body = #(\"s\")\nEXPR #(FIVE) -> Number = body"),
        "load: <test>:1:13: this body returns Str, which can never satisfy its declared return Number"
    );
}

#[test]
fn recursion_goes_through_a_mark() {
    assert_eq!(
        run("LET countdown = #((PRINT \"tick\") (\\(COUNTDOWN null)))
EXPR #(COUNTDOWN n :Number) -> Any = countdown
EXPR #(COUNTDOWN n :Null) -> Any = #(PRINT \"liftoff\")
COUNTDOWN 1"),
        "tick\nliftoff"
    );
}

#[test]
fn recursion_through_a_dollar_is_an_eager_cycle() {
    assert_eq!(
        run("UNION Nat = #{Zero: Null, Succ: Nat}
LET body = #($(COUNTDOWN n))
EXPR #(COUNTDOWN n :Nat) -> Str = body"),
        "load: <test>:2:1: these bindings need each other's values before any of them exists: \
         `body` `COUNTDOWN _`"
    );
}

/// The body is read where the callable is born, as an argument is: a body nested in a callable
/// reads its scope's later bindings and traces them, and a statement's does not see them.
#[test]
fn a_quote_written_after_its_callable_is_traced_from_a_nested_body() {
    assert_eq!(
        run(
            "EXPR #(MAKE) -> Any = #(FN :{w :Number, h :Number} -> Number = area)
LET area = #(w * h)
PRINT ((MAKE) {w = 2, h = 3})"
        ),
        "6"
    );
    assert_eq!(
        run("EXPR #(MAKE) -> Any = #(FN :{w :Number} -> Number = area)\nLET area = #(\"s\")"),
        "load: <test>:2:13: this body returns Str, which can never satisfy its declared return \
         Number"
    );
    assert_eq!(
        run("EXPR #(AREA w :Number h :Number) -> Number = area\nLET area = #(w * h)"),
        "load: <test>:1:46: `area` names no binding visible here"
    );
}

#[test]
fn a_traced_callable_registers_and_exports() {
    assert_eq!(
        run("MODULE m = (
  LET area = #(w * h)
  EXPR #(AREA w :Number h :Number) -> Number = area
)
USING m SCOPE (PRINT (AREA 2 3))"),
        "6"
    );
    assert_eq!(
        run("LET named = #(\"named\")
EXPR #(PICK x :Number) -> Str = named
EXPR #(PICK x :Str) -> Str = #(\"written\")
PRINT (PICK 1)
PRINT (PICK \"a\")"),
        "named\nwritten"
    );
}

#[test]
fn the_quote_value_keeps_its_own_type() {
    assert_eq!(
        run("LET area = #(w * \\h)
EXPR #(AREA w :Number h :Number) -> Number = area
EXPR #(NEEDS code :(Expression NEEDING #[h])) -> Str = #(\"admitted\")
PRINT area
PRINT (NEEDS area)"),
        "w * \\h\nadmitted"
    );
    assert_eq!(
        run("LET area = #(w * \\h)
EXPR #(AREA w :Number h :Number) -> Number = area
EXPR #(CLOSED code :Expression) -> Str = #(\"admitted\")
CLOSED area"),
        "load: <test>:4:1: no overload of `CLOSED _` admits (:(Expression NEEDING #[h]))"
    );
}

#[test]
fn one_quote_names_the_body_of_two_callables() {
    assert_eq!(
        run("LET body = #(FN :{y :Number} -> Number = #(y + \\w))
EXPR #(ONE w :Number) -> Any = body
EXPR #(TWO w :Number) -> Any = body
PRINT ((ONE 1) == (TWO 1))
PRINT ((ONE 1) == (TWO 2))"),
        "true\nfalse"
    );
}

#[test]
fn an_operator_body_is_named() {
    assert_eq!(
        run("LET joined = #(\"joined\")
OP #(+) OVER Str = joined
PRINT (\"a\" + \"b\")"),
        "joined"
    );
    assert_eq!(
        run("LET first = #(left)
OP #(+) OVER Str = first
PRINT (\"a\" + \"b\")"),
        "a"
    );
}

#[test]
fn a_quantified_function_has_a_named_body() {
    assert_eq!(
        run("LET ident = #(x)
EXPR FOR ALL #[Elt] #(SAME x :Elt) -> Elt = ident
PRINT (SAME 5)
PRINT (SAME \"s\")"),
        "5\ns"
    );
}

#[test]
fn a_hole_never_binds_an_enclosing_callables_name() {
    assert_eq!(
        run("LET body = #(w + k)
EXPR #(OUTER k :Number) -> Any = #(FN :{w :Number} -> Number = body)"),
        "load: <test>:1:18: `k` names no binding visible here"
    );
}

/// A declaration naming code inside a quote leaves the quote's carried type as written: the inner
/// quote's `\` mark is its own, filled from the parameter where the body is built.
#[test]
fn a_declaration_inside_a_quote_leaves_its_type_as_written() {
    assert_eq!(
        run("LET outer = #(
  (LET b = #(w * \\h))
  (EXPR #(AREA w :Number h :Number) -> Number = b)
  (AREA 2 3))
EXPR #(CLOSED code :Block) -> Str = #(\"admitted\")
PRINT (CLOSED outer)
PRINT (EVAL outer -> Any)"),
        "admitted\n6"
    );
}

/// Two bodies built over one quote each read a path through a call written in place: each its own
/// type, and both run.
#[test]
fn two_bodies_over_one_quote_read_an_inline_path() {
    let source = format!(
        "{COLLECTION}LET body = #(
  (EXPR #(MK elem :Number) -> Module = #(
    (MODULE built = (LET empty = [0]))
    (built :| \\Collection)))
  (LET x :((MK e).Set) = (MK e).empty)
  (x))
EXPR #(ONE e :Number) -> Any = body
EXPR #(TWO e :Number) -> Any = body
PRINT (ONE 1)
PRINT (TWO 1)"
    );
    assert_eq!(run(&source), "Set([0])\nSet([0])");
}

#[test]
fn a_body_reading_its_own_parameters_is_written_as_a_quote() {
    assert_eq!(
        run("LET f = (FN :{x :Number} -> Number = (x + 1))"),
        "load: <test>:1:39: `x` is a parameter, which only the body reads: write the body as a \
         quote, #(…)"
    );
}

#[test]
fn a_code_parameter_is_an_untraced_body() {
    assert_eq!(
        run("EXPR #(LIFT code :(Expression NEEDING #[x])) -> Any = #(
  FN :{x :Number} -> Number = code
)
LET inc = (LIFT #(\\x + 1))
PRINT (inc {x = 4})"),
        "5"
    );
}

#[test]
fn holes_in_untraced_code_fill_from_parameters() {
    assert_eq!(
        run("EXPR #(LIFT code :(Expression NEEDING #[x])) -> Any = #(
  FN :{x :Number} -> Number = code
)
PRINT ((LIFT #(x + 1)) {x = 4})"),
        "5"
    );
}

#[test]
fn the_declaring_scope_fills_the_names_a_parameters_type_lists() {
    assert_eq!(
        run("LET y = 10
EXPR #(LIFT code :(Expression NEEDING #[y])) -> Any = #(
  FN :{x :Number} -> Number = code
)
LET inc = (LIFT #(x + \\y))
PRINT (inc {x = 4})"),
        "14"
    );
}

#[test]
fn a_mark_the_type_does_not_list_is_unbound_at_the_first_call() {
    assert_eq!(
        run("LET y = 10
EXPR #(LIFT code :Code) -> Any = #(FN :{x :Number} -> Number = code)
LET g = (LIFT #(x + \\y))
PRINT \"born\"
PRINT (g {x = 4})"),
        "born\nerror: unbound name 'y'"
    );
}

#[test]
fn a_leftover_hole_is_unbound_at_the_first_call() {
    assert_eq!(
        run(
            "EXPR #(LIFT code :Code) -> Any = #(FN :{x :Number} -> Number = code)
LET g = (LIFT #(x + k))
PRINT \"born\"
PRINT (g {x = 4})"
        ),
        "born\nerror: unbound name 'k'"
    );
}

#[test]
fn composed_code_is_an_untraced_body() {
    assert_eq!(
        run("LET parts = [#(\\x * 2)]
LET doubled = #($..parts)
LET f = (FN :{x :Number} -> Number = doubled)
PRINT (f {x = 4})"),
        "8"
    );
}

#[test]
fn a_member_read_and_a_call_are_untraced_bodies() {
    assert_eq!(
        run("MODULE m = (LET body = #(x + 1))
LET make = (FN :{} -> Any = #(#(x * 2)))
LET f = (FN :{x :Number} -> Number = m.body)
LET g = (FN :{x :Number} -> Number = (make {}))
PRINT (f {x = 4})
PRINT (g {x = 4})"),
        "5\n8"
    );
}

#[test]
fn a_body_that_can_never_be_code_is_refused_at_load() {
    assert_eq!(
        run("LET n = 5\nLET f = (FN :{x :Number} -> Number = n)"),
        "load: <test>:2:38: this body is Number, which can never be code: write the body as a \
         quote, #(…), or name code"
    );
    assert_eq!(
        run("LET f = (FN :{} -> Number = 42)"),
        "load: <test>:1:29: this body is Number, which can never be code: write the body as a \
         quote, #(…), or name code"
    );
}

#[test]
fn a_body_found_not_code_where_the_callable_is_born_is_an_error_there() {
    assert_eq!(
        run(
            "EXPR #(LIFT code :Any) -> Any = #(FN :{x :Number} -> Number = code)
LET g = (LIFT 5)
PRINT \"unreached\""
        ),
        "error: this body is Number, which is not code for a callable to run"
    );
}

#[test]
fn untraced_bodies_of_equal_code_share_a_composition() {
    let (printed, tiers, _) = super::compositions::composed(
        "EXPR #(LIFT code :Code) -> Any = #(FN :{x :Number} -> Number = code)
LET one = (LIFT #(x + 1))
LET two = (LIFT #(x + 1))
PRINT (one == two)
PRINT (one {x = 1})
PRINT (two {x = 2})",
    );
    assert_eq!(printed, "true\n2\n3");
    assert_eq!(tiers.composed, 1);
}

#[test]
fn an_untraced_body_with_no_parameters_shares_builds_composition() {
    let (printed, tiers, _) = super::compositions::composed(
        "LET parts = [#(PRINT \"ran\")]
LET code = #($..parts)
LET f = (FN :{} -> Any = code)
f {}
EVAL (BUILD code) -> Any",
    );
    assert_eq!(printed, "ran\nran");
    assert_eq!(tiers.composed, 1);
}

#[test]
fn a_data_member_of_a_knot_holds_an_untraced_function() {
    assert_eq!(
        run("LET make = (FN :{} -> Any = #(#(x + 1)))
LET r = {f = (FN :{x :Number} -> Number = (make {})), g = g}
LET g = (FN :{} -> Any = #(r))
PRINT (r.f {x = 1})"),
        "2"
    );
}

/// Each call of an untraced function whose composition another evicted rebuilds it from the code
/// the function holds, bouncing once with its syntax.
#[test]
fn an_evicted_composition_is_rebuilt_at_the_next_call() {
    let (printed, tiers) = super::compositions::smallest(
        "EXPR #(LIFT code :Code) -> Any = #(FN :{x :Number} -> Number = code)
LET f = (LIFT #(x + 1))
LET p = #(PRINT 0)\nLET ps = [p]
PRINT (f {x = 1})
EVAL #($..ps) -> Any
PRINT (f {x = 2})",
    );
    assert_eq!(printed, "2\n0\n3");
    assert_eq!((tiers.composed, tiers.bounced), (3, 3));
}

#[test]
fn an_untraced_registration_is_born_through_the_tie() {
    assert_eq!(
        run("LET parts = [#(PRINT \"pong\")]
LET pong = #($..parts)
EXPR #(PING n :Number) -> Any = #(PONG n)
EXPR #(PONG n :Number) -> Any = pong
PING 1"),
        "pong"
    );
}

/// What a code parameter's type lists is read where the callable is born, so naming a fellow there
/// is an eager cycle, as reading one in the body part is.
#[test]
fn an_offered_fellow_is_an_eager_cycle() {
    assert_eq!(
        run(
            "EXPR #(LIFT code :(Expression NEEDING #[(PING _)])) -> Any = #(
  (EXPR #(PING n :Number) -> Any = #(PONG n))
  (EXPR #(PONG n :Number) -> Any = code)
  (PONG 1))"
        ),
        "load: <test>:2:3: these bindings need each other's values before any of them exists: \
         `PONG _` `PING _`"
    );
}

#[test]
fn quantified_and_operator_callables_have_untraced_bodies() {
    assert_eq!(
        run("LET parts = [#(x)]
LET ident = #($..parts)
EXPR FOR ALL #[Elt] #(SAME x :Elt) -> Elt = ident
PRINT (SAME 5)
PRINT (SAME \"s\")"),
        "5\ns"
    );
    assert_eq!(
        run("LET parts = [#(left)]
LET first = #($..parts)
OP #(+) OVER Str = first
PRINT (\"a\" + \"b\")"),
        "a"
    );
}

#[test]
fn untraced_code_births_a_callable_under_its_composition() {
    assert_eq!(
        run(
            "EXPR #(LIFT code :Code) -> Any = #(FN :{x :Number} -> Any = code)
LET f = (LIFT #(FN :{y :Number} -> Number = #(x + y)))
PRINT ((f {x = 1}) {y = 2})"
        ),
        "3"
    );
}

/// A `USING` fills a hole in an untraced body's part, a part of the code around it.
#[test]
fn a_using_fills_a_hole_in_an_untraced_body() {
    assert_eq!(
        run(
            "LET q = #((LET f = (FN :{x :Number} -> Number = body)) (f {x = 2}))
LET j = (q USING {body = #(x * 10)})
PRINT (EVAL j -> Any)"
        ),
        "20"
    );
    assert_eq!(
        run(
            "LET q = #((LET f = (FN :{x :Number} -> Number = (pick {n = k}))) (f {x = 2}))
LET pick = (FN :{n :Number} -> Any = #(#(x * $n)))
LET j = (q USING {k = 3, pick = pick})
PRINT (EVAL j -> Any)"
        ),
        "6"
    );
}

#[test]
fn a_module_exports_an_untraced_registration() {
    assert_eq!(
        run("MODULE m = (
  LET parts = [#(\\x * 2)]
  LET doubled = #($..parts)
  EXPR #(TWICE x :Number) -> Number = doubled
)
USING m SCOPE (PRINT (TWICE 4))"),
        "8"
    );
}

/// What untraced code refuses where it is built, and a result its signature refuses, are errors
/// of the call.
#[test]
fn an_untraced_body_refuses_at_its_call() {
    assert_eq!(
        run(
            "EXPR #(LIFT code :Code) -> Any = #(FN :{x :Number} -> Number = code)
LET f = (LIFT #(x + \"s\"))
PRINT \"born\"
PRINT (f {x = 1})"
        ),
        "born\nerror: no overload of `_ + _` admits (Number, Str)"
    );
    assert_eq!(
        run(
            "EXPR #(LIFT code :Code) -> Any = #(FN :{x :Number} -> Number = code)
PRINT ((LIFT #(\"s\")) {x = 1})"
        ),
        "error: :(FN :{x :Number} -> Number) returned Str, which does not satisfy Number"
    );
}

/// A `FOR ALL` name is a parameter, and fills a hole of the code, traced or not.
#[test]
fn a_quantifier_fills_a_hole() {
    assert_eq!(
        run("LET body = #(Elt)
EXPR FOR ALL #[Elt] #(KIND x :Elt) -> Any = body
PRINT (KIND 1)"),
        "Number"
    );
    assert_eq!(
        run("LET parts = [#(Elt)]
LET body = #($..parts)
EXPR FOR ALL #[Elt] #(KIND x :Elt) -> Any = body
PRINT (KIND 1)"),
        "Number"
    );
}

#[test]
fn a_key_a_code_parameters_type_lists_is_offered_to_untraced_code() {
    assert_eq!(
        run("EXPR #(GREET who :Number) -> Str = #(\"hi\")
EXPR #(LIFT code :(Expression NEEDING #[(GREET _)])) -> Any = #(
  FN :{x :Number} -> Any = code
)
PRINT ((LIFT #(\\(GREET x))) {x = 1})"),
        "hi"
    );
}
