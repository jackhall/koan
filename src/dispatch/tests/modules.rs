//! Module programs run end to end: an ascription under each operator, a member read, a type member
//! read through a module, and a quantified member read where a type is wanted — at load, typed and
//! refused, and at run, read and faulted where the load could not see.

use crate::type_lattice::display_name;

use super::run;
use super::statics::{interval, loaded};

/// `SIG Counter`, and `MODULE ints`, which fits it.
const COUNTER: &str = "SIG Counter FOR ALL #[Carrier] = \
                       #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                       MODULE ints = ((LET zero = 0) \
                       (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n";

/// `SIG HasLabel`, a module with a label and one without.
const LABELS: &str = "SIG HasLabel = #[(VAL label :Str)]\n\
                      MODULE named = ((LET label = \"n\") (LET other = 1))\n\
                      MODULE plain = (LET other = 1)\n";

#[test]
fn a_transparent_view_shows_what_each_parameter_solves_to() {
    let source = format!(
        "{COUNTER}LET t = (ints :! Counter)\n\
         PRINT t\n\
         PRINT (t.succ {{x = t.zero}})\n\
         PRINT t.Carrier"
    );
    assert_eq!(
        run(&source),
        "SIG (Carrier: Number, zero: Number, succ: :(FN :{x :Number} -> Number))\n1\nNumber"
    );
}

#[test]
fn an_opaque_view_seals_each_member_behind_its_carrier() {
    let source = format!(
        "{COUNTER}LET v = (ints :| Counter)\n\
         PRINT v\n\
         PRINT v.zero\n\
         PRINT v.Carrier"
    );
    assert_eq!(
        run(&source),
        "SIG (Carrier: Carrier, zero: Carrier, succ: :(FN :{x :Carrier} -> Carrier))\n\
         Carrier(0)\n\
         Carrier"
    );
}

#[test]
fn a_member_is_read_by_name_through_a_chain_and_by_a_quoted_label() {
    let source = "MODULE outer = ((LET value = 1) (MODULE inner = (LET value = 2)))\n\
                  PRINT outer.value\n\
                  PRINT outer.inner.value\n\
                  LET which = #(value)\n\
                  PRINT (ATTR outer (which))";
    assert_eq!(run(source), "1\n2\n1");
}

#[test]
fn a_type_member_is_read_through_a_module_name() {
    let source = "MODULE ints = (LET Carrier = Number)\n\
                  PRINT ints.Carrier\n\
                  LET x :(ints.Carrier) = 1\n\
                  PRINT x\n\
                  MODULE outer = (MODULE inner = (LET Elt = Str))\n\
                  LET y :(outer.inner.Elt) = \"s\"\n\
                  PRINT y";
    assert_eq!(run(source), "Number\n1\ns");
    assert_eq!(
        run("MODULE ints = (LET Carrier = Number)\nPRINT ints.Nope"),
        "error: SIG (Carrier: Number) has no member Nope"
    );
}

#[test]
fn a_missing_member_and_an_ascription_that_can_never_hold_refuse_the_load() {
    assert_eq!(
        run("MODULE geometry = (LET pi = 3)\nPRINT geometry.tau"),
        "load: <test>:2:7: SIG (pi: Number) has no member tau"
    );
    assert_eq!(
        run(&format!("{LABELS}PRINT (plain :! HasLabel)")),
        "load: <test>:4:7: this value is SIG (other: Number), which can never satisfy its \
         ascription SIG (label: Str)"
    );
    assert_eq!(
        run(&format!(
            "{LABELS}EXPR #(TAKE m :HasLabel) -> Str = #(m.label)\nPRINT (TAKE named)\n\
             PRINT (TAKE plain)"
        )),
        "load: <test>:6:7: no overload of `TAKE _` admits (SIG (other: Number))"
    );
}

#[test]
fn what_the_load_cannot_see_faults_at_run() {
    let hide = "EXPR #(HIDE x :Any) -> Any = #(x)\n";
    assert_eq!(
        run(&format!(
            "{hide}MODULE geometry = (LET pi = 3)\nLET g = (HIDE geometry)\nPRINT g.tau"
        )),
        "error: SIG (pi: Number) has no member tau"
    );
    assert_eq!(
        run(&format!(
            "{hide}{LABELS}LET p = (HIDE plain)\nPRINT (p :! HasLabel)"
        )),
        "error: SIG (other: Number) does not satisfy its ascription SIG (label: Str)"
    );
    assert_eq!(
        run(&format!(
            "{hide}MODULE m = (LET pick = (FN FOR ALL #[Elt] :{{x :Elt}} -> Elt = #(x)))\n\
             LET h = (HIDE m)\nLET g :Any = h.pick\nPRINT \"read\""
        )),
        "error: member pick is quantified, and no type was known to instantiate it at"
    );
    assert_eq!(
        run("PRINT (1 :| Any)"),
        "error: Number is no module to ascribe"
    );
}

#[test]
fn a_module_binder_is_exactly_the_signature_the_run_ties() {
    let source = "SIG Box = #[(VAL x :Number)]\n\
                  MODULE m = ((LET x = 1) (LET Elt = Str) (LET f = (FN :{y :Number} -> Number = \
                  #(y))) (EXPR #(HELP z :Number) -> Number = #(z)) \
                  (MODULE inner = (LET x = 2)) \
                  (LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))))\n";
    let at_load = loaded(source, |program| {
        let typed = interval(program, program.shape(), "m");
        assert!(typed.is_exact(), "every member is exact");
        display_name(typed.upper, program.types(), program.symbols()).to_string()
    });
    assert_eq!(run(&format!("{source}PRINT m")), at_load);
    // A member whose static type is at most another makes the binder at most `Module`.
    let inexact = "MODULE m = (LET x = (EVAL #(1) -> Any))";
    loaded(inexact, |program| {
        let typed = interval(program, program.shape(), "m");
        assert!(!typed.is_exact());
        assert_eq!(
            display_name(typed.upper, program.types(), program.symbols()).to_string(),
            "Module"
        );
    });
}

/// `MODULE m` binding a quantified `pick`.
const PICK: &str = "MODULE m = (LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x)))\n";

#[test]
fn a_quantified_member_is_instantiated_where_it_is_wanted() {
    assert_eq!(
        run(&format!(
            "{PICK}LET inc :(FN :{{x :Number}} -> Number) = m.pick\nPRINT inc\nPRINT (inc {{x = 3}})"
        )),
        ":(FN :{x :Number} -> Number)\n3"
    );
    assert_eq!(
        run(&format!(
            "{PICK}LET h = (FN :{{f :(FN :{{x :Bool}} -> Bool)}} -> Bool = #(f {{x = true}}))\n\
             PRINT (h {{f = m.pick}})"
        )),
        "true",
        "a call by name's argument, wanted at its parameter"
    );
    assert_eq!(
        run(&format!(
            "{PICK}EXPR #(USE f :(FN :{{x :Str}} -> Str)) -> Str = #(f {{x = \"k\"}})\n\
             PRINT (USE m.pick)"
        )),
        "k",
        "a keyworded argument, wanted at its slot"
    );
    assert_eq!(run(&format!("{PICK}PRINT (m.pick {{x = 1}})")), "1");
    assert_eq!(
        run(&format!("{PICK}LET g = m.pick")),
        "load: <test>:2:9: nothing fixes `Elt` here: a quantified function is read only at the \
         head of a call, as the binding of a `MODULE` member, or where the type it is wanted at \
         solves its group"
    );
}

/// A quantified member over a head parameter its signature leaves unpinned is refused anywhere but
/// a call's head, where the run solves the call.
#[test]
fn a_quantified_member_over_an_unpinned_parameter_is_read_only_at_a_call_s_head() {
    let applier = "SIG Applier FOR ALL #[Carrier] = \
                   #[(VAL apply :(FN FOR ALL #[Elt] :{x :Elt, c :Carrier} -> Elt))]\n";
    assert_eq!(
        run(&format!(
            "{applier}EXPR #(USEIT p :Applier) -> Any = \
             #(LET w :(FN :{{x :Str, c :Number}} -> Str) = p.apply)"
        )),
        "load: <test>:2:79: `apply` is quantified over `Carrier`, which the load cannot name \
         here; call it, or ascribe its module with `Carrier` pinned"
    );
    assert_eq!(
        run(&format!(
            "{applier}EXPR #(USEIT p :Applier) -> Any = #(p.apply {{x = 1, c = 2}})\nPRINT \"ok\""
        )),
        "ok"
    );
}

#[test]
fn a_using_block_opens_a_module_s_names_and_definitions() {
    assert_eq!(
        run("MODULE greetings = (LET hello = \"hi there\")\nPRINT (USING greetings SCOPE (hello))"),
        "hi there"
    );
    assert_eq!(
        run(
            "MODULE doubling = (LET dbl = FN EXPR #(DOUBLE x :Number) -> Number = #(x))\n\
             PRINT (USING doubling SCOPE (DOUBLE 21))"
        ),
        "21",
        "a combined definition's registration"
    );
    assert_eq!(
        run(
            "MODULE palette = (UNION Color = #{Red: Null, Blue: Null})\n\
             PRINT (USING palette SCOPE (\
             (EXPR #(DESCRIBE c :Color) -> Str = #(\"a color\")) (DESCRIBE (Color.Red null))))"
        ),
        "a color"
    );
    assert_eq!(
        run("MODULE counter = (LET step = 2)\n\
             PRINT (USING counter SCOPE ((LET half = step) (half + step)))"),
        "4"
    );
    assert_eq!(
        run("SIG Doubler = #[(EXPR #(DOUBLE _ :Number) -> Number)]\n\
             MODULE doubling = (EXPR #(DOUBLE x :Number) -> Number = #(x * 2))\n\
             LET doubles = (doubling :| Doubler)\n\
             PRINT (USING doubles SCOPE (DOUBLE 21))"),
        "42",
        "a view's keyworded member"
    );
}

#[test]
fn a_quantified_head_is_called_through_a_view_at_each_type() {
    let source = "SIG Boxes = #[(EXPR FOR ALL #[Elt] #(BOX _ :Elt) -> :(LIST OF Elt))]\n\
                  MODULE boxing = (EXPR FOR ALL #[Elt] #(BOX x :Elt) -> :(LIST OF Elt) = #([x]))\n\
                  PRINT (USING (boxing :| Boxes) SCOPE (BOX 7))\n\
                  PRINT (USING (boxing :! Boxes) SCOPE (BOX \"hi\"))";
    assert_eq!(run(source), "[7]\n[hi]");
}

#[test]
fn a_using_block_spreads_every_overload_at_a_key() {
    let two = "MODULE two = ((EXPR #(PICK x :Number) -> Str = #(\"number\")) \
               (EXPR #(PICK x :Str) -> Str = #(\"str\")) \
               (EXPR #(PICK x :Any) -> Str = #(\"any\")))\n";
    assert_eq!(
        run(&format!(
            "{two}PRINT (USING two SCOPE (PICK 1))\nPRINT (USING two SCOPE (PICK \"s\"))\n\
             PRINT (USING two SCOPE (PICK null))"
        )),
        "number\nstr\nany"
    );
    let tied = "MODULE tied = ((EXPR #(PICK x :Number) -> Str = #(\"a\")) \
                (EXPR #(PICK x :(Number | Str)) -> Str = #(\"b\")))\n";
    assert_eq!(
        run(&format!("{tied}PRINT (USING tied SCOPE (PICK \"s\"))")),
        "b"
    );
    let ambiguous = "MODULE both = ((EXPR #(PICK x :(Number | Str)) -> Str = #(\"a\")) \
                     (EXPR #(PICK x :(Number | Bool)) -> Str = #(\"b\")))\n";
    assert_eq!(
        run(&format!("{ambiguous}PRINT (USING both SCOPE (PICK 1))")),
        "error: ambiguous call of PICK _: 2 overloads admit (Number) and none ranks first"
    );
}

#[test]
fn a_call_through_an_opaque_view_crosses_its_barrier_both_ways() {
    let source = format!(
        "{COUNTER}LET v = (ints :| Counter)\n\
         PRINT (v.succ {{x = v.zero}})\n\
         PRINT (v.succ {{x = (v.succ {{x = v.zero}})}})"
    );
    assert_eq!(run(&source), "Carrier(1)\nCarrier(2)");
    assert_eq!(
        run(&format!(
            "{COUNTER}LET v = (ints :| Counter)\nPRINT (v.succ {{x = 1}})"
        )),
        "error: :(FN :{x :Carrier} -> Carrier) cannot be called with :{x :Number}: it is not \
         sealed under Carrier",
        "an argument the view never sealed"
    );
    let stepping = "SIG Stepper FOR ALL #[Carrier] = \
                    #[(VAL zero :Carrier) (EXPR #(STEP _ :Carrier) -> Carrier)]\n\
                    MODULE twos = ((LET zero = 0) (EXPR #(STEP x :Number) -> Number = #(x + 2)))\n\
                    LET s = (twos :| Stepper)\n";
    assert_eq!(
        run(&format!(
            "{stepping}PRINT (USING s SCOPE (STEP zero))\nPRINT (USING s SCOPE (STEP (STEP zero)))"
        )),
        "Carrier(2)\nCarrier(4)",
        "a keyworded member behind a barrier"
    );
    // A tail call through a barrier keeps its frame, so its value crosses the barrier.
    assert_eq!(
        run(&format!(
            "{COUNTER}LET v = (ints :| Counter)\n\
             LET bump = (FN :{{}} -> Any = #(v.succ {{x = v.zero}}))\nPRINT (bump {{}})"
        )),
        "Carrier(1)"
    );
}

/// A head parameter's bound decides which modules fit, and nothing more: outside the view a sealed
/// member is opaque, so no `Number` slot, builtin or annotation takes it, while the view's own
/// functions see its payload behind their barriers, and the view still fits its signature.
#[test]
fn a_sealed_member_is_opaque_outside_its_view_whatever_its_bound() {
    let source = |program: &str| {
        format!(
            "SIG Counter FOR ALL #{{Carrier: Number}} = \
             #[(VAL zero :Carrier) (VAL succ :(FN :{{x :Carrier}} -> Carrier))]\n\
             MODULE ints = ((LET zero = 4) (LET succ = (FN :{{x :Number}} -> Number = #(x + 1))))\n\
             LET c = (ints :| Counter)\n\
             EXPR #(USE m :Counter) -> Any = #(m.zero + 1)\n\
             EXPR #(STEP m :Counter) -> Any = #(m.succ {{x = m.zero}})\n\
             EXPR #(SHOW n :Number) -> Any = #(n)\n\
             SIG Plain = #[(VAL zero :Number)]\n\
             EXPR #(TAKE m :Plain) -> Number = #(m.zero + 1)\n\
             EXPR #(PASS p :Counter) -> Any = #(TAKE p)\n{program}"
        )
    };
    let refused_plus = "error: no overload of _ + _ admits (Carrier, Number)";
    let refused_take = "error: no overload of TAKE _ admits (SIG (Carrier: Carrier, zero: \
                        Carrier, succ: :(FN :{x :Carrier} -> Carrier)))";
    for (program, printed) in [
        ("PRINT (c.zero + 1)", refused_plus),
        (
            "PRINT (SHOW c.zero)",
            "error: no overload of SHOW _ admits (Carrier)",
        ),
        (
            "LET n :Number = c.zero\nPRINT n",
            "error: Carrier does not satisfy its annotation Number",
        ),
        ("PRINT (c.zero == 4)", "false"),
        ("PRINT (c.succ {x = c.zero})", "Carrier(5)"),
        ("PRINT (USE ints)", "5"),
        ("PRINT (USE c)", refused_plus),
        ("PRINT (STEP c)", "Carrier(5)"),
        // A bounded parameter offers no `Number`, so neither the view nor a `Counter` fits `Plain`.
        ("PRINT (TAKE ints)", "5"),
        ("PRINT (TAKE c)", refused_take),
        ("PRINT (PASS c)", refused_take),
    ] {
        assert_eq!(run(&source(program)), printed, "{program}");
    }
}

/// An opaque view of an opaque view reseals each member under its own carrier, and a call through
/// it crosses both barriers each way; a value sealed under the inner view is no value of the outer.
#[test]
fn an_opaque_view_of_an_opaque_view_reseals_and_stacks_its_barriers() {
    let source = |program: &str| {
        format!(
            "SIG Counter FOR ALL #[Carrier] = \
             #[(VAL zero :Carrier) (VAL succ :(FN :{{x :Carrier}} -> Carrier))]\n\
             SIG Again FOR ALL #[Carrier] = \
             #[(VAL zero :Carrier) (VAL succ :(FN :{{x :Carrier}} -> Carrier))]\n\
             MODULE ints = ((LET zero = 4) (LET succ = (FN :{{x :Number}} -> Number = #(x + 1))))\n\
             LET c = (ints :| Counter)\nLET d = (c :| Again)\n{program}"
        )
    };
    assert_eq!(
        run(&source("PRINT (d.succ {x = (d.succ {x = d.zero})})")),
        "Carrier(6)"
    );
    assert_eq!(
        run(&source("PRINT (d.succ {x = c.zero})")),
        "error: :(FN :{x :Carrier} -> Carrier) cannot be called with :{x :Carrier}: its value \
         does not satisfy Carrier"
    );
}

/// The Miri slate's barrier call: a list crosses a barrier inwards, and one built from a captured
/// string crosses it outwards, twice over.
#[test]
fn a_call_by_name_through_a_barrier_rebuilds_its_value_both_ways() {
    let source = "SIG Stepper FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL step :(FN :{x :Carrier} -> Carrier))]\n\
                  MODULE named = ((LET tag = \"seen\") (LET zero = [\"z\"]) \
                  (LET step = (FN :{x :(LIST OF Str)} -> :(LIST OF Str) = #([tag]))))\n\
                  LET v = (named :| Stepper)\n\
                  PRINT (v.step {x = (v.step {x = v.zero})})";
    assert_eq!(run(source), "Carrier([seen])");
}

/// The Miri slate's `USING` block: a keyworded member behind a barrier, reached by its surfaced
/// key, over a surfaced value member.
#[test]
fn a_using_block_calls_a_surfaced_member_through_its_barrier() {
    let source = "SIG Stepper FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (EXPR #(STEP _ :Carrier) -> Carrier)]\n\
                  MODULE twos = ((LET tag = \"two\") (LET zero = [\"z\"]) \
                  (EXPR #(STEP x :(LIST OF Str)) -> :(LIST OF Str) = #([tag])))\n\
                  LET s = (twos :| Stepper)\n\
                  PRINT (USING s SCOPE (STEP (STEP zero)))";
    assert_eq!(run(source), "Carrier([two])");
}

#[test]
fn an_argument_crossing_a_union_slot_inwards_unseals_by_the_member_over_the_carrier() {
    let source = "SIG Shown FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL show :(FN :{x :(Carrier | Str)} -> Str))]\n\
                  MODULE m = ((LET zero = 0) \
                  (LET show = (FN :{x :(Number | Str)} -> Str = #(\"shown\"))))\n\
                  LET v = (m :| Shown)\n";
    assert_eq!(
        run(&format!(
            "{source}PRINT (v.show {{x = v.zero}})\nPRINT (v.show {{x = \"s\"}})"
        )),
        "shown\nshown"
    );
    assert_eq!(
        run(&format!("{source}PRINT (v.show {{x = 1}})")),
        "error: :(FN :{x :(Carrier | Str)} -> Str) cannot be called with :{x :Number}: no member \
         of its union admits it"
    );
}

/// What `source` refuses at load with, where it refuses an unlisted `name`.
fn unlisted(source: &str, name: &str) {
    let refused = run(source);
    assert!(
        refused.starts_with("load: ")
            && refused.contains(&format!("`{name}` is read from outside this module")),
        "`{source}` refuses `{name}` unlisted, not: {refused}"
    );
}

#[test]
fn a_module_reading_an_outer_name_lists_it_under_over() {
    // A parameter and a local of the enclosing callable.
    let reads = |over: &str| {
        format!(
            "EXPR #(MK p :Number) -> Any = #(\n  LET loc = 1\n  \
             MODULE m{over} = (LET x = (p + loc))\n  m.x\n)\nPRINT (MK 2)"
        )
    };
    assert_eq!(run(&reads(" OVER #[p loc]")), "3");
    unlisted(&reads(""), "p");
    unlisted(&reads(" OVER #[p]"), "loc");
    // A type the enclosing callable binds is listed as a value is.
    let typed = |over: &str| {
        format!(
            "EXPR FOR ALL #[Elt] #(MK p :Elt) -> Any = #(\n  \
             MODULE m{over} = (LET x :Elt = p)\n  m.x\n)\nPRINT (MK 2)"
        )
    };
    assert_eq!(run(&typed(" OVER #[p Elt]")), "2");
    unlisted(&typed(" OVER #[p]"), "Elt");
    // A registration, by its key.
    let keyed = |over: &str| {
        format!(
            "EXPR #(MK p :Number) -> Any = #(\n  \
             (EXPR #(HELPER z :Number) -> Number = #(z * 10))\n  \
             MODULE m{over} = (LET x = (HELPER p))\n  m.x\n)\nPRINT (MK 2)"
        )
    };
    assert_eq!(run(&keyed(" OVER #[p (HELPER _)]")), "20");
    assert!(
        run(&keyed(" OVER #[p]")).ends_with(
            "`HELPER _` is read from outside this module; list it under its `OVER`, as \
             `OVER #[(HELPER _)]`"
        ),
        "an unlisted registration is named by its key"
    );
    // A name only a function nested in the body reads.
    let nested = |over: &str| {
        format!(
            "EXPR #(MK p :Number) -> Any = #(\n  \
             MODULE m{over} = (LET f = (FN :{{}} -> Number = #(p)))\n  m.f {{}}\n)\nPRINT (MK 4)"
        )
    };
    assert_eq!(run(&nested(" OVER #[p]")), "4");
    unlisted(&nested(""), "p");
}

#[test]
fn a_top_level_name_is_read_where_it_lives() {
    assert_eq!(
        run("LET y = 5\nMODULE m = (LET x = y)\nPRINT m.x"),
        "5",
        "a top-level read needs no list"
    );
    assert_eq!(
        run("LET y = 5\nMODULE m OVER #[y (PRINT _)] = (LET x = y)\nPRINT m.x"),
        "5",
        "a top-level name or a builtin may be listed"
    );
    assert_eq!(
        run("MODULE m OVER #[nowhere] = (LET x = 1)"),
        "load: <test>:1:1: `nowhere` names no binding visible here"
    );
}

#[test]
fn a_nested_module_and_an_eval_offer_fall_under_the_contract() {
    let nested = |inner: &str| {
        format!(
            "EXPR #(MK p :Number) -> Any = #(\n  \
             MODULE outer OVER #[p] = (MODULE inner{inner} = (LET x = p))\n  outer.inner.x\n)\n\
             PRINT (MK 3)"
        )
    };
    assert_eq!(run(&nested(" OVER #[p]")), "3");
    unlisted(&nested(""), "p");
    let offered = |over: &str| {
        format!(
            "EXPR #(RUN c :(Expression NEEDING #[y]) y :Number) -> Any = #(\n  \
             MODULE m{over} = (LET v = (EVAL c -> Any))\n  m.v\n)\n\
             PRINT (RUN #(\\y + 1) 4)"
        )
    };
    assert_eq!(run(&offered(" OVER #[c y]")), "5");
    unlisted(&offered(" OVER #[c]"), "y");
}

/// A quote's open hole is no outer name: what fills it — a builtin, or a code `USING` — binds before
/// the code runs, so a module in the code reads it unlisted. A local of the code is outer, as a
/// callable's is.
#[test]
fn a_module_in_a_quote_reads_the_code_s_holes_unlisted() {
    let run_made = |code: &str, filled: &str| {
        run(&format!(
            "LET code = #({code})\nLET made = (EVAL {filled} -> Any)\nPRINT made.x"
        ))
    };
    assert_eq!(run_made("MODULE m = (LET x = (1 + 2))", "code"), "3");
    assert_eq!(
        run_made("MODULE m = (LET x = y)", "(code USING {y = 5})"),
        "5"
    );
    // Code's shapes are built where it runs, so the refusal waits for the `EVAL`.
    assert_eq!(
        run("LET code = #((LET y = 1) (MODULE m = (LET x = y)))\nEVAL code -> Any"),
        "error: <test>:1:26: `y` is read from outside this module; list it under its `OVER`, \
         as `OVER #[y]`"
    );
}

#[test]
fn a_group_body_lists_what_it_reads_after_its_name() {
    let groups = [
        "GROUP g{over} FOLD LEFT = (OP #(<+>) OVER Number = #(left + right + k))",
        "GROUP g{over} FOLD RIGHT = (OP #(<+>) OVER Number = #(left + right + k))",
        "GROUP g{over} PAIRWISE FOLD #(AND) LEFT = \
         (OP #(<+>) OVER Number -> Bool = #(left < (right + k)))",
        "GROUP g{over} PAIRWISE FOLD #(AND) RIGHT = \
         (OP #(<+>) OVER Number -> Bool = #(left < (right + k)))",
    ];
    for group in groups {
        let program = |over: &str| {
            format!(
                "EXPR #(MK k :Number) -> Any = #(\n  {}\n  null\n)\nPRINT (MK 1)",
                group.replace("{over}", over)
            )
        };
        assert_eq!(run(&program(" OVER #[k]")), "null", "{group}");
        unlisted(&program(""), "k");
    }
}

/// A type a call's contribution reads, though the body names it nowhere, is a capture the list
/// must name.
#[test]
fn a_type_a_contribution_reads_is_listed_too() {
    let program = |over: &str| {
        format!(
            "EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str = #(\"pair\")\n\
             EXPR FOR ALL #[Outer] #(MK a :Outer) -> Any = #(\n  \
             MODULE m OVER #[{over}] = (LET v = (PAIR a WITH a))\n  m.v\n)\nPRINT (MK 1)"
        )
    };
    assert_eq!(run(&program("a Outer")), "pair");
    unlisted(&program("a"), "Outer");
}

/// A carrier is keyed on content: the ascribed module's, the signature application's and the
/// parameter's name — never on when or how often the ascription runs.
#[test]
fn a_carrier_is_keyed_on_content() {
    assert_eq!(
        run(&format!(
            "{COUNTER}LET a = (ints :| Counter)\nLET b = (ints :| Counter)\n\
             PRINT (a.Carrier == b.Carrier)\nPRINT (a.succ {{x = b.zero}})"
        )),
        "true\nCarrier(1)",
        "two evaluations of one ascription share their carrier"
    );
    assert_eq!(
        run(&format!(
            "{COUNTER}MODULE twin = ((LET zero = 0) \
             (LET succ = (FN :{{x :Number}} -> Number = #(x + 1))))\n\
             MODULE other = ((LET zero = 0) \
             (LET succ = (FN :{{x :Number}} -> Number = #(x + 2))))\n\
             LET a = (ints :| Counter)\nLET b = (twin :| Counter)\nLET c = (other :| Counter)\n\
             PRINT (a.Carrier == b.Carrier)\nPRINT (a.Carrier == c.Carrier)\n\
             PRINT (a.succ {{x = c.zero}})"
        )),
        "true\nfalse\nerror: :(FN :{x :Carrier} -> Carrier) cannot be called with \
         :{x :Carrier}: it is not sealed under Carrier",
        "modules of equal content share a carrier, and of other content do not"
    );
    assert_eq!(
        run(
            "SIG Pair FOR ALL #[Left Right] = #[(VAL l :Left) (VAL r :Right)]\n\
             MODULE p = ((LET l = 1) (LET r = 2))\nLET v = (p :| Pair)\n\
             PRINT (v.Left == v.Right)"
        ),
        "false",
        "two unpinned parameters of one signature"
    );
}

#[test]
fn a_functor_s_carrier_follows_what_its_module_captures() {
    let source = "SIG Ordered = #[(VAL compare :Number)]\n\
                  SIG Set FOR ALL #[Elt] = #[(VAL empty :Elt)]\n\
                  MODULE ascending = (LET compare = 1)\n\
                  MODULE descending = (LET compare = 2)\n\
                  EXPR #(MAKESET elem :Ordered) -> Module = #(\n  \
                  MODULE built OVER #[elem] = (LET empty = elem.compare)\n  \
                  built :| Set\n)\n\
                  LET up = (MAKESET ascending)\nLET again = (MAKESET ascending)\n\
                  LET down = (MAKESET descending)\n\
                  PRINT (up.Elt == again.Elt)\nPRINT (up.Elt == down.Elt)";
    assert_eq!(run(source), "true\nfalse");
    let unread = "SIG Ord FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
                  LET mk = (FN :{x :Any, u :Any} -> Any = #(\n  \
                  MODULE m OVER #[x u] = (LET zero = x)\n  m :| Ord\n))\n\
                  LET a = (mk {x = 1, u = 1})\nLET b = (mk {x = 1, u = 2})\n\
                  PRINT (a.Carrier == b.Carrier)";
    assert_eq!(run(unread), "false", "an unread `OVER` entry is content");
    let branded = "SIG Ord FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
                   LET metric = 1\nLET imperial = 2\n\
                   MODULE meters OVER #[metric] = (LET zero = 0)\n\
                   MODULE feet OVER #[imperial] = (LET zero = 0)\n\
                   LET m = (meters :| Ord)\nLET f = (feet :| Ord)\n\
                   PRINT (m.Carrier == f.Carrier)";
    assert_eq!(
        run(branded),
        "false",
        "one body listing two top-level names"
    );
}
