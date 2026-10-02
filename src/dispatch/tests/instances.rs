//! Instance sites: a quantified function made concrete where the type it is wanted at fixes its
//! group — an annotation, an ascription, a body's declared return, a container's element type —
//! and refused where nothing does.

use super::run;
use super::statics::{binder, loaded, module_body, top};

const PICK: &str = "(LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x)))";

/// `MODULE lib = (<pick> <rest>)`: `rest` beside the quantified member `pick`.
fn module(rest: &str) -> String {
    format!("MODULE lib = ({PICK} {rest})")
}

const UNFIXED: &str = "nothing fixes `Elt` here: a quantified function is read only at the head of \
                       a call, as the binding of a `MODULE` member, or where the type it is wanted \
                       at solves its group";

#[test]
fn a_quantified_binding_outside_a_module_is_refused_whether_or_not_read() {
    assert_eq!(
        run("LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))"),
        format!("load: <test>:1:1: {UNFIXED}")
    );
    assert_eq!(run(&module("(PRINT \"loaded\")")), "loaded");
}

#[test]
fn an_annotated_binding_instantiates_its_quantified_value() {
    let inc =
        "LET inc :(FN :{x :Number} -> Number) = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))\n";
    assert_eq!(run(&format!("{inc}PRINT (inc {{x = 1}})")), "1");
    assert_eq!(top(inc, "inc"), ":(FN :{x :Number} -> Number)");
    assert_eq!(
        run(&format!("{inc}PRINT (inc {{x = \"s\"}})")),
        "load: <test>:2:7: :(FN :{x :Number} -> Number) can never be called with :{x :Str}"
    );
    assert_eq!(
        run(&format!(
            "LET f = (FN :{{}} -> Number = #(({}) (inc {{x = 5}})))\nPRINT (f {{}})",
            inc.trim_end()
        )),
        "5"
    );
}

#[test]
fn a_module_member_is_instantiated_where_it_is_wanted() {
    assert_eq!(
        run(&module(
            "(LET inc :(FN :{x :Number} -> Number) = pick) \
             (LET keep :(LIST OF (FN :{x :Number} -> Number)) = [pick]) \
             (PRINT (inc {x = 1})) \
             (PRINT keep)"
        )),
        "1\n[:(FN :{x :Number} -> Number)]"
    );
    assert_eq!(
        run(&module(
            "(LET s = (pick :! :(FN :{x :Str} -> Str))) (PRINT s) (PRINT (s {x = \"a\"}))"
        )),
        ":(FN :{x :Str} -> Str)\na"
    );
}

#[test]
fn a_member_nothing_fixes_is_refused_and_named() {
    assert_eq!(
        run(&module("(LET keep = [pick])")),
        format!("load: <test>:1:84: {UNFIXED}")
    );
    assert_eq!(
        run(
            "LET f :(FN :{x :Number} -> Number) = (FN FOR ALL #[Elt] :{x :Number} -> Number = #(x))"
        ),
        "load: <test>:1:1: :(FN :{x :Number} -> Number) does not fix `Elt`"
    );
    for wanted in [":(FN :{} -> Null)", ":(FN :{x :Number} -> Str)"] {
        let source = module(&format!("(LET f {wanted} = pick)"));
        let column = source.rfind("pick").expect("the read is written") + 1;
        assert_eq!(
            run(&source),
            format!(
                "load: <test>:1:{column}: :(FN FOR ALL #[Elt] :{{x :Elt}} -> Elt) has no instance \
                 under {wanted}"
            )
        );
    }
}

#[test]
fn a_body_s_declared_return_instantiates_its_last_binding() {
    let source = "LET f = (FN :{} -> :(FN :{x :Number} -> Number) = \
                  #(LET g = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))))\n";
    assert_eq!(
        run(&format!(
            "{source}PRINT (f {{}})\nPRINT ((f {{}}) {{x = 7}})"
        )),
        ":(FN :{x :Number} -> Number)\n7"
    );
}

#[test]
fn an_instance_is_used_like_any_unquantified_function() {
    let kind = "EXPR #(KIND f :(FN :{x :Number} -> Number)) -> Str = #(\"numbers\")\n\
                EXPR #(KIND f :Any) -> Str = #(\"any\")\n";
    assert_eq!(
        run(&format!(
            "{kind}{}",
            module(
                "(LET inc :(FN :{x :Number} -> Number) = pick) \
                 (PRINT inc) (PRINT (KIND inc)) (PRINT ((FN :{} -> Any = #(inc)) {}))"
            )
        )),
        ":(FN :{x :Number} -> Number)\nnumbers\n:(FN :{x :Number} -> Number)"
    );
}

#[test]
fn a_quantified_literal_is_born_as_its_instance() {
    assert_eq!(
        run(
            "LET s = ((FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x)) :! :(FN :{x :Str} -> Str))\n\
             PRINT s\nPRINT (s {x = \"b\"})"
        ),
        ":(FN :{x :Str} -> Str)\nb"
    );
    assert_eq!(
        run("LET keep = [(FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))]"),
        format!("load: <test>:1:13: {UNFIXED}")
    );
}

#[test]
fn an_instance_solving_a_run_bound_type_is_refused() {
    assert_eq!(
        run("EXPR FOR ALL #[Outer] #(WRAP y :Outer) -> Any = #(\n  \
             LET f :(FN :{x :Outer} -> Outer) = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))\n)"),
        "load: <test>:1:50: this solves `Elt` to a type each run binds; ascribe a type no `FOR ALL` \
         names"
    );
}

#[test]
fn an_instance_body_reads_its_solution_and_calls_its_siblings() {
    let source = "MODULE lib = (\
                  (LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #((PRINT Elt) (other {x = x})))) \
                  (LET other = (FN :{x :Any} -> Any = \
                  #((LET unused = (FN :{} -> Any = #(pick {x = 1}))) (x)))) \
                  (LET inc :(FN :{x :Number} -> Number) = pick) \
                  (PRINT (inc {x = 9})))";
    assert_eq!(run(source), "Number\n9");
}

#[test]
fn instances_are_equal_at_one_solution() {
    assert_eq!(
        run(&module(
            "(LET a :(FN :{x :Number} -> Number) = pick) \
             (LET b :(FN :{x :Number} -> Number) = pick) \
             (LET c :(FN :{x :Str} -> Str) = pick) \
             (PRINT (a == b)) (PRINT (a == c))"
        )),
        "true\nfalse"
    );
}

#[test]
fn a_keyworded_argument_is_instantiated_at_its_slot() {
    let twice = "EXPR #(TWICE f :(FN :{x :Number} -> Number) AT y :Number) -> Number = \
                 #(f {x = (f {x = y})})\n";
    assert_eq!(
        run(&format!(
            "{twice}{}",
            module("(PRINT (TWICE pick AT 1)) (PRINT (TWICE (pick) AT 2))")
        )),
        "1\n2"
    );
    let apply = "EXPR FOR ALL #[Elt] #(APPLY f :(FN :{x :Elt} -> Elt) TO y :Elt) -> Elt = \
                 #(f {x = y})\n";
    assert_eq!(
        run(&format!(
            "{apply}{}\nPRINT (APPLY (FN FOR ALL #[Item] :{{x :Item}} -> Item = #(x)) TO 3)",
            module("(PRINT (APPLY pick TO 1))")
        )),
        "1\n3"
    );
}

#[test]
fn a_quantified_slot_is_solved_from_the_other_arguments_first() {
    let map = "EXPR FOR ALL #[Elt Out] #(MAP f :(FN :{x :Elt} -> Out) AT y :Elt) -> Out = \
               #(f {x = y})\n";
    let source = format!("{map}{}", module("(LET m = (MAP pick AT 1)) (PRINT m)"));
    assert_eq!(run(&source), "1");
    loaded(&source, |program| {
        assert_eq!(binder(program, module_body(program, "lib"), "m"), "Number");
    });
    let apply = "EXPR FOR ALL #[Elt] #(APPLY f :(FN :{x :Elt} -> Elt) TO y :Elt) -> Elt = \
                 #(f {x = y})\n";
    let source = format!(
        "{apply}{}",
        module("(LET g = (FN :{y :(Number | Str)} -> Any = #(APPLY pick TO y)))")
    );
    let column = source.rfind("pick").expect("the argument is written") + 1;
    assert_eq!(
        run(&source),
        format!(
            "load: <test>:2:{}: {UNFIXED}",
            column - source.rfind('\n').unwrap() - 1
        )
    );
}

#[test]
fn candidates_take_an_instance_argument_at_their_own_slots() {
    let using = "EXPR #(USE f :(FN :{x :Number} -> Number) WITH y :Number) -> Str = #(\"numbers\")\n\
                 EXPR #(USE f :(FN :{x :Str} -> Str) WITH y :Str) -> Str = #(\"strings\")\n";
    assert_eq!(
        run(&format!("{using}{}", module("(PRINT (USE pick WITH 1))"))),
        "numbers"
    );
    let take = "EXPR #(TAKE f :(FN :{x :Number} -> Number) AND y :Number) -> Str = #(\"n\")\n\
                EXPR #(TAKE f :(FN :{x :Str} -> Str) AND y :Str) -> Str = #(\"s\")\n";
    assert_eq!(
        run(&format!(
            "{take}{}",
            module("(LET g = (FN :{y :(Number | Str)} -> Any = #(TAKE pick AND y)))")
        )),
        "load: <test>:3:116: the overloads of `TAKE _ AND _` this call keeps want this function at \
         different types; ascribe it"
    );
}

#[test]
fn a_container_argument_takes_no_wanted_type_unless_ascribed() {
    let keep = "EXPR #(KEEP fs :(LIST OF (FN :{x :Number} -> Number))) -> Str = #(\"kept\")\n";
    assert_eq!(
        run(&format!("{keep}{}", module("(KEEP [pick])"))),
        format!("load: <test>:2:78: {UNFIXED}")
    );
    assert_eq!(
        run(&format!(
            "{keep}{}",
            module("(PRINT (KEEP ([pick] :! :(LIST OF (FN :{x :Number} -> Number)))))")
        )),
        "kept"
    );
}

#[test]
fn a_call_by_name_pushes_its_parameters_to_its_record() {
    assert_eq!(
        run(&module(
            "(LET use = (FN :{f :(FN :{g :(FN :{x :Number} -> Number)} -> Number)} -> Number = \
             #(f {g = pick}))) \
             (PRINT (use {f = (FN :{g :(FN :{x :Number} -> Number)} -> Number = #(g {x = 3}))}))"
        )),
        "3"
    );
    assert_eq!(
        run(&module(
            "(LET apply = (FN FOR ALL #[Item] :{f :(FN :{x :Item} -> Item), y :Item} -> Item = \
             #(f {x = y}))) \
             (PRINT (apply {f = pick, y = 4}))"
        )),
        "4"
    );
}

/// A Miri slate test: an instance of a member of a two-member knot whose closure holds an edge to
/// its sibling and a captured string, made at a frame's last statement and called after the frame
/// hands it back.
#[test]
fn an_instance_made_in_a_frame_is_called_where_it_lands() {
    let source = "MODULE lib = (\
                  (LET tag = \"seen\") \
                  (LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #((PRINT tag) (other {x = x})))) \
                  (LET other = (FN :{x :Any} -> Any = \
                  #((LET unused = (FN :{} -> Any = #(pick {x = 1}))) (x)))) \
                  (LET mk = (FN :{} -> :(FN :{x :Number} -> Number) = #(pick))) \
                  (LET got = (mk {})) \
                  (PRINT (got {x = 9})))";
    assert_eq!(run(source), "seen\n9");
}

#[test]
fn a_shared_variable_another_argument_leaves_open_is_refused() {
    let map = "EXPR FOR ALL #[Elt Out] #(MAP f :(FN :{x :Elt} -> Out) AT y :Elt) -> Out = \
               #(f {x = y})\n";
    let source = format!(
        "{map}{}",
        module("(LET g = (FN :{y :(Number | Str)} -> Any = #(MAP pick AT y)))")
    );
    let line = &source[source.rfind('\n').unwrap() + 1..];
    let column = line.rfind("pick").expect("the argument is written") + 1;
    assert_eq!(run(&source), format!("load: <test>:2:{column}: {UNFIXED}"));
}

#[test]
fn other_arguments_that_never_fit_drop_the_candidate() {
    let pair = "EXPR FOR ALL #[Elt] #(PAIR f :(FN :{x :Elt} -> Elt) WITH y :(LIST OF Elt)) -> Elt = \
                #(f {x = 1})\n";
    // The quantified argument is listed as `Any`: no instance of it is made.
    assert_eq!(
        run(&format!("{pair}{}", module("(PAIR pick WITH 1)"))),
        "load: <test>:2:72: no overload of `PAIR _ WITH _` admits (Any, Number)"
    );
}

#[test]
fn an_instance_bound_by_name_keeps_its_exact_type() {
    let source = module("(LET inc :(FN :{x :Number} -> Number) = pick) (PRINT (inc {x = \"s\"}))");
    let column = source.rfind("(inc").expect("the call is written") + 1;
    assert_eq!(
        run(&source),
        format!(
            "load: <test>:1:{column}: :(FN :{{x :Number}} -> Number) can never be called with \
             :{{x :Str}}"
        )
    );
}

#[test]
fn an_imprecise_other_argument_fixes_nothing() {
    let source = module(
        "(LET ap = (FN FOR ALL #[Item] :{f :(FN :{x :Item} -> Item), y :(LIST OF Item)} -> Any = \
         #(f {x = 1}))) \
         (LET g = (FN :{z :Any} -> Any = #(ap {f = pick, y = z})))",
    );
    // Located at the record literal, the nearest spanned part.
    let column = source.find("{f = pick").expect("the record is written") + 1;
    let unfixed = UNFIXED.replace("`Elt`", "`Item`");
    assert_eq!(run(&source), format!("load: <test>:1:{column}: {unfixed}"));
    let pair = "EXPR FOR ALL #[Elt] #(PAIR f :(FN :{x :Elt} -> Elt) WITH y :(LIST OF Elt)) -> Elt = \
                #(f {x = 1})\n";
    let source = format!(
        "{pair}{}",
        module("(LET g = (FN :{z :Any} -> Any = #(PAIR pick WITH z)))")
    );
    let line = &source[source.rfind('\n').expect("two lines") + 1..];
    let column = line.rfind("pick").expect("the argument is written") + 1;
    assert_eq!(run(&source), format!("load: <test>:2:{column}: {UNFIXED}"));
}

#[test]
fn a_union_wanted_type_fixes_nothing_and_renders_as_written() {
    let source = module("(LET f :((FN :{x :Number} -> Number) | Null) = pick)");
    let column = source.rfind("pick").expect("the read is written") + 1;
    assert_eq!(
        run(&source),
        format!(
            "load: <test>:1:{column}: :((FN :{{x :Number}} -> Number) | Null) does not fix `Elt`"
        )
    );
}
