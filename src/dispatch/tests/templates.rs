//! String templates: a `%"…"` literal evaluates to its text with each `{…}` brace's value rendered
//! as `PRINT` renders it, or, with a `%{…}` brace, to the record of the strings around the kept
//! values and those values; a quoted template fills its holes as any quote does; and `JOIN`
//! splices a list's rendered elements, with a separator after `BY`.

use super::run;

#[test]
fn a_template_renders_each_brace_into_its_text() {
    let source = "LET name = \"bob\"\nLET view = (JOIN [\"ab\" \"cd\"])\n\
                  PRINT %\"Hello, {name}!\"\n\
                  PRINT %\"{1 + 2} {[1 'a']} { {x = 1} } {view} { ({a = 1}).a }\"\n\
                  PRINT %\"{{x}} %{{\"\nPRINT %\"a\"\nPRINT %''";
    assert_eq!(
        run(source),
        "Hello, bob!\n3 [1, a] {x = 1} abcd 1\n{x} %{\na\n"
    );
}

#[test]
fn a_kept_brace_makes_a_record_of_the_strings_and_the_kept_values() {
    let source = "LET x = 1\nLET y = 2\nPRINT %\"a{x}b%{y}c\"\nPRINT %\"%{x}-%{y}\"";
    assert_eq!(
        run(source),
        "{strings = [a1b, c], values = [2]}\n{strings = [, -, ], values = [1, 2]}"
    );
}

#[test]
fn a_consumer_selects_on_the_kept_values_types() {
    let consumer = "EXPR #(SHOW t :{strings :(LIST OF Str), values :(LIST OF Number)}) -> Any = #(\n\
                    \x20 PRINT (t.values 0)\n\x20 PRINT (t.strings 1)\n)\n";
    assert_eq!(
        run(&format!("{consumer}SHOW %\"n = %{{1}}!\"")),
        ":(Ok {Error = Never, Ok = Number})(1)\n:(Ok {Error = Never, Ok = Str})(!)"
    );
    assert!(
        run(&format!("{consumer}SHOW %\"s = %{{'x'}}\"")).starts_with("load: "),
        "a kept string is no number"
    );
}

#[test]
fn the_load_types_a_template_as_a_string_or_its_record() {
    use super::statics::top;
    assert_eq!(top("LET r = %\"a{1}\"", "r"), "Str");
    assert_eq!(
        top("LET r = %\"a%{1}\"", "r"),
        ":{strings :(LIST OF Str) values :(LIST OF Number)}"
    );
    assert_eq!(run("LET t :Str = %\"{1}\"\nPRINT t"), "1");
    assert!(run("LET t :Str = %\"%{1}\"").starts_with("load: "));
}

#[test]
fn a_brace_reads_names_where_the_template_is() {
    assert_eq!(
        run("EXPR #(GREET who :Str) -> Str = #(%\"Hi {who}\")\nPRINT (GREET \"ann\")"),
        "Hi ann"
    );
    assert_eq!(
        run("LET greet = #(%\"Hi {name}\")\nPRINT (EVAL (greet USING {name = \"bob\"}) -> Str)"),
        "Hi bob"
    );
    assert_eq!(
        run("LET x = 5\nLET q = #(%\"x = {$x}\")\nPRINT (EVAL q -> Str)"),
        "x = 5"
    );
    assert_eq!(
        run("LET greet = #(%\"Hi {name}\")\nPRINT (EVAL greet -> Str)"),
        "error: unbound name 'name'"
    );
    assert_eq!(
        run("LET x = 3\nLET q = #(%\"%{$x}{x}\")\nPRINT (EVAL (q USING {x = 9}) -> Any)"),
        "{strings = [, 9], values = [3]}"
    );
    assert!(run("LET s = %\"{s}\"").starts_with("load: "));
}

#[test]
fn a_template_reads_and_keys_as_its_string() {
    assert_eq!(
        run("PRINT (%\"ab{1}\" 2)\nPRINT (LENGTH %\"héllo{1}\")\nPRINT {%\"k{1}\": 2}"),
        ":(Ok {Error = Never, Ok = Str})(1)\n6\n{\"k1\": 2}"
    );
}

#[test]
fn join_splices_a_list_s_rendered_elements() {
    let source = "PRINT (JOIN [\"a\" 1 [2]])\nPRINT (JOIN [])\n\
                  PRINT (JOIN [\"a\" \"b\" \"c\"] BY \", \")\nPRINT (JOIN [\"x\"] BY \", \")";
    assert_eq!(run(source), "a1[2]\n\na, b, c\nx");
    assert!(run("PRINT (JOIN [1 2] BY 3)").starts_with("load: "));
}
