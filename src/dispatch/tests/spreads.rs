//! Spreads: `..xs` lays a list's elements into a list literal one level deep, as a view; the load
//! types the literal from its spread lists' element types and refuses a spread of what can never
//! be a list, and the run faults on one it could not refuse.

use super::run;

#[test]
fn a_spread_lays_a_list_s_elements_into_a_list_literal() {
    let source = "LET xs = [1 2]\nLET ys = [\"b\"]\n\
                  PRINT [..xs 3]\nPRINT [..xs ..ys]\nPRINT [..([]) ..xs]\nPRINT [0 ..xs 0]\n\
                  PRINT ([..xs 3] == [1 2 3])\nPRINT (LENGTH [..xs ..ys ..xs])\n\
                  PRINT ([..xs \"a\"] {from = 1})";
    assert_eq!(
        run(source),
        "[1, 2, 3]\n[1, 2, b]\n[1, 2]\n[0, 1, 2, 0]\ntrue\n5\n\
         :(Ok {Error = Never, Ok = :(LIST OF :(Number | Str))})([2, a])"
    );
}

#[test]
fn a_spread_of_what_is_no_list_is_refused_where_the_load_can_tell_and_faults_otherwise() {
    assert_eq!(
        run("PRINT [..(5)]"),
        "load: <test>:1:7: `..` spreads a list, and this value is Number, which can never be one"
    );
    assert_eq!(
        run("LET f = (FN :{v :Any} -> Any = #([..v]))\nPRINT (f {v = 5})"),
        "error: `..` spreads a list, and Number is none"
    );
}

#[test]
fn the_load_types_a_spread_literal_from_its_lists_element_types() {
    use super::statics::top;
    let typed = |line: &str| top(&format!("LET xs = [1 2]\nLET ys = [\"b\"]\n{line}"), "r");
    assert_eq!(typed("LET r = [..xs 3]"), ":(LIST OF Number)");
    assert_eq!(typed("LET r = [..xs ..ys]"), ":(LIST OF :(Number | Str))");
}

#[test]
fn a_quote_spreading_a_list_is_other_code_than_one_holding_it() {
    assert_eq!(
        run("LET xs = [1]\nPRINT (#([..xs]) == #([xs]))\nPRINT (#([..xs]) == #([..xs]))"),
        "false\ntrue"
    );
}

#[test]
fn a_recursive_list_spreads_lists_from_outside_its_group_and_lists_within_it() {
    let length = "(FN :{} -> Number = #(LENGTH xs))";
    assert_eq!(
        run(&format!(
            "LET ys = [1 2]\nLET xs = [..ys {length}]\n\
             PRINT xs\nPRINT (LENGTH xs)\nPRINT (xs 0)\nPRINT [..xs 0]"
        )),
        format!(
            "[1, 2, :(FN :{{}} -> Number)]\n3\n{}\n[1, 2, :(FN :{{}} -> Number), 0]",
            ":(Ok {Error = Never, Ok = Number})(1)"
        )
    );
    // A fellow member spread, read through its edge, and a fellow function calling into the view.
    assert_eq!(
        run(&format!(
            "LET xs = [..zs 1]\nLET zs = [f 7]\nLET f = {length}\n\
             PRINT xs\nPRINT (f {{}})\nPRINT (xs {{from = 1}})"
        )),
        format!(
            "[:(FN :{{}} -> Number), 7, 1]\n3\n{}",
            ":(Ok {Error = Never, Ok = :(LIST OF :((FN :{} -> Number) | Number))})([7, 1])"
        )
    );
    assert_eq!(
        run(&format!(
            "LET xs = [[..ys] {length}]\nLET ys = [1 {length}]\nPRINT xs"
        )),
        "[[1, :(FN :{} -> Number)], :(FN :{} -> Number)]"
    );
}

#[test]
fn a_spread_cycle_or_a_spread_of_no_list_refuses_its_group() {
    assert_eq!(
        run("LET xs = [..zs 1]\nLET zs = [..xs 2]\nPRINT xs"),
        "error: these bindings build values of no finite type: `xs` `zs`"
    );
    let not_a_list = "error: `..` in `xs` spreads what is not a list";
    let length = "(FN :{} -> Number = #(LENGTH xs))";
    assert_eq!(
        run(&format!(
            "LET xs = [..zs 1]\nLET zs = {{a = {length}}}\nPRINT xs"
        )),
        not_a_list
    );
    assert_eq!(
        run(&format!(
            "LET v :Any = 5\nLET xs = [..v {length}]\nPRINT xs"
        )),
        not_a_list
    );
    assert_eq!(
        run(&format!("LET xs = [..f 1]\nLET f = {length}\nPRINT xs")),
        "load: <test>:1:10: `..` spreads a list, and this value is :(FN :{} -> Number), which can \
         never be one"
    );
}

#[test]
fn a_recursive_list_view_crosses_out_of_its_frame() {
    let source = "LET mk = (FN :{n :Number} -> Any = #((LET ys = [n n]) \
                  (LET xs = [..ys (FN :{} -> Number = #(LENGTH xs))]) (xs)))\n\
                  LET r = (mk {n = 4})\nPRINT r\nPRINT (LENGTH r)\nPRINT [..r ..r]";
    assert_eq!(
        run(source),
        "[4, 4, :(FN :{} -> Number)]\n3\n\
         [4, 4, :(FN :{} -> Number), 4, 4, :(FN :{} -> Number)]"
    );
}
