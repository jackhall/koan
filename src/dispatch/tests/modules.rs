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
