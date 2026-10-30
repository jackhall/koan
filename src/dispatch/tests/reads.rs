//! Reads: a list, a dict or a string at an application's head read by its argument, the builtin
//! `Result` every read answers, the load's typing and refusals, and `LENGTH`.

use super::run;

#[test]
fn the_builtin_result_is_bound_and_its_variants_construct() {
    assert_eq!(
        run("PRINT (Result.Ok 1)\nPRINT (Result.Error \"boom\")"),
        ":(Ok {Error = Never, Ok = Number})(1)\n:(Error {Error = Str, Ok = Never})(boom)"
    );
}

#[test]
fn a_result_slot_admits_either_variant_and_refuses_a_payload_outside_it() {
    let declared = "LET f = (FN :{r :(Result {Ok = Number, Error = Str})} -> Str = #(\"yes\"))\n";
    assert_eq!(
        run(&format!(
            "{declared}PRINT (f {{r = (Result.Ok 1)}})\nPRINT (f {{r = (Result.Error \"x\")}})"
        )),
        "yes\nyes"
    );
    assert!(
        run(&format!("{declared}PRINT (f {{r = (Result.Error 1)}})"))
            .starts_with("load: <test>:2:7:")
    );
}

/// The rendered `Ok` and `Error` of a read, as `PRINT` writes them.
fn ok(ok: &str, payload: &str) -> String {
    format!(":(Ok {{Error = Never, Ok = {ok}}})({payload})")
}

fn error(error: &str, payload: &str) -> String {
    format!(":(Error {{Error = {error}, Ok = Never}})({payload})")
}

#[test]
fn a_list_or_a_string_applied_to_a_number_reads_the_element_there() {
    let source = "LET xs = [10 20 30]\n\
                  PRINT (xs 0)\nPRINT (xs 2)\nPRINT (xs 3)\nPRINT (xs -1)\nPRINT (xs 1.5)\n\
                  PRINT (\"héllo\" 1)\nPRINT (\"héllo\" 5)";
    let expected = [
        ok("Number", "10"),
        ok("Number", "30"),
        error("Number", "3"),
        error("Number", "-1"),
        error("Number", "1.5"),
        ok("Str", "é"),
        error("Number", "5"),
    ];
    assert_eq!(run(source), expected.join("\n"));
}

#[test]
fn a_slice_record_slices_forward_or_backward_and_answers_the_first_bound_out_of_range() {
    let list = ":(LIST OF Number)";
    let source = "LET xs = [10 20 30]\n\
                  PRINT (xs {from = 1})\n\
                  PRINT (xs {to = 2})\n\
                  PRINT (xs {from = 1, to = 2, other = \"ignored\"})\n\
                  PRINT (xs {from = (LENGTH xs)})\n\
                  PRINT ([7] {from = 1})\n\
                  PRINT (xs {from = 2, to = -1})\n\
                  PRINT (xs {from = ((LENGTH xs) - 1), to = -1})\n\
                  PRINT ([] {from = -1, to = -1})\n\
                  PRINT (xs {from = -1, to = -1})\n\
                  PRINT (xs {from = 2, to = 0})\n\
                  PRINT (xs {from = -1, to = 2})\n\
                  PRINT (xs {from = 3, to = 0})\n\
                  PRINT (xs {from = 0, to = 4})\n\
                  PRINT (xs {from = 0.5, to = 9.5})\n\
                  PRINT (\"héllo\" {from = 4, to = -1})\n\
                  PRINT (\"\" {from = -1, to = -1})\n\
                  PRINT (\"héllo\" {from = 1, to = 3})";
    let expected = [
        ok(list, "[20, 30]"),
        ok(list, "[10, 20]"),
        ok(list, "[20]"),
        ok(list, "[]"),
        ok(list, "[]"),
        ok(list, "[30, 20, 10]"),
        ok(list, "[30, 20, 10]"),
        ok(":(LIST OF Never)", "[]"),
        ok(list, "[]"),
        ok(list, "[30, 20]"),
        error("Number", "-1"),
        error("Number", "3"),
        error("Number", "4"),
        error("Number", "0.5"),
        ok("Str", "olléh"),
        ok("Str", ""),
        ok("Str", "él"),
    ];
    assert_eq!(run(source), expected.join("\n"));
}

#[test]
fn a_record_naming_neither_bound_is_refused_where_the_load_can_tell_and_faults_otherwise() {
    assert_eq!(
        run("PRINT ([1 2] {x = 1})"),
        "load: <test>:1:7: :(LIST OF Number) can never be read by :{x :Number}"
    );
    assert_eq!(
        run("PRINT ([1] {from = \"a\"})"),
        "load: <test>:1:7: :(LIST OF Number) can never be read by :{from :Str}"
    );
    assert_eq!(
        run("LET f = (FN :{v :Any} -> Any = #([1 2] v))\nPRINT (f {v = {x = 1}})"),
        "error: :(LIST OF Number) is read by a `Number` or by a slice record naming `from` or \
         `to` as `Number`s, not by :{x :Number}"
    );
}

#[test]
fn a_dict_applied_to_a_key_looks_it_up() {
    let source = "LET d = {\"a\": 1, \"b\": 2}\n\
                  PRINT (d \"b\")\nPRINT (d \"z\")\n\
                  LET f = (FN :{k :Any} -> Any = #(d k))\n\
                  PRINT (f {k = 7})\nPRINT (f {k = \"a\"})\nPRINT (f {k = [1]})";
    let expected = [
        ok("Number", "2"),
        error("Str", "z"),
        error("Number", "7"),
        ok("Number", "1"),
        "error: :(LIST OF Number) cannot be a dict key".to_string(),
    ];
    assert_eq!(run(source), expected.join("\n"));
    assert_eq!(
        run("PRINT ({\"a\": 1} 5)"),
        "load: <test>:1:7: :(MAP Str -> Number) can never be read by Number"
    );
    assert_eq!(
        run("PRINT ({\"a\": 1} [1])"),
        "load: <test>:1:7: :(MAP Str -> Number) can never be read by :(LIST OF Number)"
    );
}

#[test]
fn length_counts_elements_entries_and_code_points() {
    let source = "LET xs = [1 2 3]\n\
                  PRINT (LENGTH xs)\nPRINT (LENGTH [])\nPRINT (LENGTH {\"a\": 1})\n\
                  PRINT (LENGTH \"héllo\")\n\
                  LET r = (xs {from = 1})\n\
                  PRINT r";
    assert_eq!(
        run(source),
        format!("3\n0\n1\n5\n{}", ok(":(LIST OF Number)", "[2, 3]"))
    );
}

#[test]
fn the_load_types_a_read_from_both_ends_of_its_head() {
    use super::statics::top;
    let read = |source: &str| {
        top(
            &format!("LET xs = [1 \"a\"]\nLET d = {{\"k\": 1}}\n{source}"),
            "r",
        )
    };
    let result = |ok: &str, error: &str| {
        format!("(Ok {{Error = {error}, Ok = {ok}}}) | (Error {{Error = {error}, Ok = {ok}}})")
    };
    let element = ":(Number | Str)";
    assert_eq!(
        read("LET r = (xs 0)"),
        format!(":({})", result(element, "Number"))
    );
    assert_eq!(
        read("LET r = (xs {from = 1})"),
        format!(":({})", result(":(LIST OF :(Number | Str))", "Number"))
    );
    assert_eq!(
        read("LET r = (d \"k\")"),
        format!(":({})", result("Number", "Str"))
    );
    assert_eq!(
        read("LET f = (FN :{k :Any} -> Any = #(d k))\nLET r = (d (f {k = 1}))"),
        format!(":({})", result("Number", "Any"))
    );
}

#[test]
fn a_read_s_result_satisfies_its_declared_result_and_a_list_no_function_type() {
    assert_eq!(
        run(
            "LET f = (FN :{r :(Result {Ok = Number, Error = Number})} -> Str = #(\"yes\"))\n\
             PRINT (f {r = ([1 2] 0)})"
        ),
        "yes"
    );
    assert!(
        run(
            "LET g = (FN :{f :(FN :{x :Number} -> Number)} -> Str = #(\"called\"))\n\
             PRINT (g {f = [1 2]})"
        )
        .starts_with("load: <test>:2:7:")
    );
}

#[test]
fn a_list_holding_a_function_that_captures_it_is_read_through_its_knot() {
    let source = "LET xs = [(FN :{} -> Number = #(LENGTH xs)) 2 3]\n\
                  PRINT (LENGTH xs)\nPRINT (xs 1)\nPRINT (xs {from = 2, to = -1})";
    let list = ":(LIST OF :((FN :{} -> Number) | Number))";
    assert_eq!(
        run(source),
        format!(
            "3\n{}\n{}",
            ok("Number", "2"),
            ok(list, "[3, 2, :(FN :{} -> Number)]")
        )
    );
}

#[test]
fn a_read_of_a_flat_string_scans_its_bytes_once() {
    use crate::values::SCANS;
    let scans = |read: &str| {
        let before = SCANS.with(|scans| scans.get());
        let printed = run(&format!("LET s = \"héllo wörld\"\nPRINT (s {read})"));
        (printed, SCANS.with(|scans| scans.get()) - before)
    };
    assert_eq!(scans("4"), (ok("Str", "o"), 1));
    assert_eq!(scans("40"), (error("Number", "40"), 1));
    assert_eq!(scans("{from = 1, to = 4}"), (ok("Str", "éll"), 1));
    assert_eq!(scans("{from = 10, to = -1}"), (ok("Str", "dlröw olléh"), 1));
}

/// Code is read as a list is: a part by index — a group part as the node it wraps — and a slice by
/// its bounds, each unbuilt code; an index past the last is an error; `LENGTH` counts its root's
/// parts.
#[test]
fn code_is_read_by_its_parts_as_unbuilt_code() {
    let source = "LET k = 2\nLET q = #((LET x = 1) (PRINT (x + $k)) 7)\n\
                  PRINT (LENGTH q)\nPRINT (q 1)\nPRINT (q 2)\nPRINT (q 3)\n\
                  PRINT (q {from = 1})\nPRINT (q {from = 2, to = -1})\nPRINT (q {from = 1, to = 1})";
    assert_eq!(
        run(source),
        [
            "3".to_string(),
            ok(":(UNBUILT Expression)", "PRINT (x + $k)"),
            ok(":(UNBUILT Literal)", "7"),
            ":(Error {Error = Number, Ok = Never})(3)".to_string(),
            ok(":(UNBUILT Expression)", "(PRINT (x + $k)) 7"),
            ok(":(UNBUILT Expression)", "7 (PRINT (x + $k)) (LET x = 1)"),
            ok(":(UNBUILT Expression)", ""),
        ]
        .join("\n")
    );
}

/// The load types a read of code as code, whatever part it names.
#[test]
fn the_load_types_a_read_of_code_as_code() {
    use super::statics::top;
    let result = "(Ok {Error = Number, Ok = Code}) | (Error {Error = Number, Ok = Code})";
    for read in ["(q 0)", "(q {from = 1})"] {
        assert_eq!(
            top(
                &format!("LET q = #((PRINT 1) (PRINT 2))\nLET r = {read}"),
                "r"
            ),
            format!(":({result})")
        );
    }
}

/// A read of code crosses its frame laid flat, its `$` names' bindings beside it, and reads and
/// compares as it did.
#[test]
fn a_read_of_code_crosses_its_frame_flat() {
    let source = "LET k = 2\nLET f = (FN :{} -> Any = #((LET q = #((PRINT 1) (PRINT $k))) (q {from = 1})))\n\
                  LET r = (f {})\nPRINT r\nPRINT (r == (f {}))";
    assert_eq!(
        run(source),
        [
            ok(":(UNBUILT Expression)", "(PRINT $k)"),
            "true".to_string()
        ]
        .join("\n")
    );
}

/// A part read out of a quote is the code of that part alone, a name its binder filled a hole
/// again: it equals a splice of the quote written of it, which is unbuilt as the read is.
#[test]
fn a_part_of_code_equals_the_quote_written_of_it_alone() {
    let source = "LET q = #((LET x = 4) (PRINT x))\nLET xs = [#(PRINT x)]\n\
                  PRINT ((q 1) == (Result.Ok #($..xs)))\n\
                  LET ys = [#(PRINT y)]\nPRINT ((q 1) == (Result.Ok #($..ys)))";
    assert_eq!(run(source), "true\nfalse");
}
