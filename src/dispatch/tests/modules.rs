//! Module programs run end to end: an ascription under each operator, a member read, a type member
//! read through a module, and a quantified member read where a type is wanted — at load, typed and
//! refused, and at run, read and faulted where the load could not see.

use crate::type_lattice::display_name;

use super::run;
use super::statics::{interval, loaded};
use crate::memory::Bump;
use crate::values::tests::reading;

/// `SIG Counter`, and `MODULE ints`, which fits it.
const COUNTER: &str = "SIG Counter FOR ALL #[Carrier] = \
                       #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                       MODULE ints = ((LET zero = 0) \
                       (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n";

/// `SIG HasLabel`, a module with a label and one without.
const LABELS: &str = "SIG HasLabel = #[(VAL label :Str)]\n\
                      MODULE named = ((LET label = \"n\") (LET other = 1))\n\
                      MODULE plain = (LET other = 1)\n";

/// `:!` re-stamps a module at the application it fits, each head parameter pinned at what *fits*
/// solves.
#[test]
fn a_retype_at_a_signature_shows_what_each_parameter_solves_to() {
    let source = format!(
        "{COUNTER}LET t = (ints :! Counter)\n\
         PRINT t\n\
         PRINT (t.succ {{x = t.zero}})\n\
         PRINT t.Carrier"
    );
    assert_eq!(
        run(&source),
        "SIG FOR ALL #{Carrier: Any} (zero: Carrier, succ: :(FN :{x :Carrier} -> Carrier)) \
         WITH {Carrier = Number}\n1\nNumber"
    );
}

/// A module under a meet re-stamps at the meet of the applications it fits, each pinned, and reads
/// each member at the meet of what the applications declaring it declare.
#[test]
fn a_retype_at_a_meet_pins_each_application() {
    let source = "SIG Aa = #[(VAL x :Number)]\n\
                  SIG Cc FOR ALL #[Elt] = #[(VAL first :Elt)]\n\
                  MODULE m = ((LET x = 1) (LET first = \"f\") (LET z = 3))\n\
                  LET both = (m :! (Aa & Cc))\n\
                  PRINT both\nPRINT both.x\nPRINT both.first\nPRINT both.Elt";
    assert_eq!(
        run(source),
        "(SIG FOR ALL #{Elt: Any} (first: Elt) WITH {Elt = Str}) & SIG (x: Number)\n1\nf\nStr"
    );
    assert_eq!(
        run(&format!("{source}\nPRINT both.z")),
        "load: <test>:9:7: (SIG FOR ALL #{Elt: Any} (first: Elt) WITH {Elt = Str}) & \
         SIG (x: Number) has no member z",
        "a member neither application declares"
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

/// A path inside a declaration's definition reads the module member, as one in an annotation does.
#[test]
fn a_definition_reads_a_type_member_through_a_module_name() {
    let module = "MODULE m = (LET Inner = Number)\n";
    for (program, printed) in [
        (
            "NEWTYPE Wrapped = :(m.Inner)\nPRINT (Wrapped 3)",
            "Wrapped(3)",
        ),
        (
            "NEWTYPE Held = :{a :(m.Inner | Null)}\nPRINT (Held {a = 3})",
            "Held({a = 3})",
        ),
        (
            "SIG Slot = #[(VAL x :(m.Inner))]\nPRINT Slot",
            "SIG (x: Number)",
        ),
        (
            "LET Alias = :(LIST OF m.Inner)\nPRINT Alias",
            ":(LIST OF Number)",
        ),
    ] {
        assert_eq!(run(&format!("{module}{program}")), printed, "{program}");
    }
}

/// A type path through a view reads each member at the type the view's signature declares it: a
/// nested module declared at an application reads its pinned head parameter, as `v.inner` then
/// `.Elem` does. The load sees the view exactly, so a value that can never be that type refuses it.
#[test]
fn a_type_path_through_a_view_reads_the_member_a_stepwise_read_does() {
    let program = |value: &str| {
        format!(
            "SIG Inner FOR ALL #[Elem] = #[(VAL zero :Elem)]\n\
             SIG Outer = #[(VAL inner :(Inner WITH {{Elem = Str}}))]\n\
             MODULE m = (MODULE inner = ((LET Elem = Str) (LET zero = \"z\")))\n\
             LET v = (m :! Outer)\n\
             LET y :(v.inner.Elem) = {value}\n\
             PRINT y\n\
             LET i = v.inner\n\
             PRINT i.Elem"
        )
    };
    assert_eq!(run(&program("\"s\"")), "s\nStr");
    assert_eq!(
        run(&program("1")),
        "load: <test>:5:1: this value is Number, which can never satisfy its annotation Str"
    );
}

/// A view the view door refuses at every run refuses the load: an operand that can never be a
/// module, and a type that is no one application of a signature. A retype at a meet is no view.
#[test]
fn a_view_every_run_refuses_refuses_the_load() {
    assert_eq!(
        run("PRINT (1 :| Any)"),
        "load: <test>:1:7: Number is no module to ascribe"
    );
    assert_eq!(
        run("SIG Aa = #[(VAL x :Number)]\n\
             SIG Bb = #[(VAL y :Number)]\n\
             MODULE m = ((LET x = 1) (LET y = 2))\n\
             PRINT (m :| (Aa & Bb))"),
        "load: <test>:4:7: SIG (y: Number) & SIG (x: Number) is no signature"
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
        "load: <test>:6:7: no overload of `TAKE _` admits this argument:\n  SIG (other: Number), \
         which can never satisfy SIG (label: Str)"
    );
}

#[test]
fn a_member_a_parameter_s_parametric_signature_lacks_refuses_the_load_uncalled() {
    let counter = "SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
                   MODULE ints = ((LET Carrier = Number) (LET zero = 0) (LET extra = \"e\"))\n";
    assert_eq!(
        run(&format!(
            "{counter}LET peek = (FN :{{m :Counter}} -> Any = #(m.extra))\nPRINT \"start\""
        )),
        "load: <test>:3:40: SIG FOR ALL #{Carrier: Any} (zero: Carrier) has no member extra"
    );
    // What it declares loads; so does a type member it lacks, which the run refuses, as it does
    // through a module the load sees whole; and through `:Module`, the argument decides.
    assert_eq!(
        run(&format!(
            "{counter}LET zero = (FN :{{m :Counter}} -> Any = #(m.zero))\n\
             LET carrier = (FN :{{m :Counter}} -> Any = #(m.Carrier))\n\
             LET extra = (FN :{{m :Module}} -> Any = #(m.extra))\n\
             LET missing = (FN :{{m :Counter}} -> Any = #(m.Missing))\n\
             PRINT (zero {{m = ints}})\nPRINT (carrier {{m = ints}})\nPRINT (extra {{m = ints}})\n\
             PRINT (missing {{m = ints}})"
        )),
        "0\nNumber\ne\nerror: SIG FOR ALL #{Carrier: Any} (zero: Carrier) WITH {Carrier = Number} \
         has no member Missing\n  in :(FN :{m :SIG FOR ALL #{Carrier: Any} (zero: Carrier)} -> \
         Any) (missing {m = ints}) at <test>:10:7"
    );
    // A type member a `:Module` parameter's module lacks refuses where the argument decides it.
    assert_eq!(
        run(&format!(
            "{counter}LET missing = (FN :{{m :Module}} -> Any = #(\n  LET Ks = :(m.Missing)\n  1\n))\n\
             PRINT (missing {{m = ints}})"
        )),
        "error: SIG (Carrier: Number, zero: Number, extra: Str) has no member Missing\n  \
         in :(FN :{m :Module} -> Any) (missing {m = ints}) at <test>:7:7"
    );
}

/// A module binder no instance of whose signature declares what a slot's signature asks for can
/// never fill it, so the call refuses at load; a parameter of a meet of signatures is pinned at
/// its paths member by member, so a member every instance lacks refuses at load too.
#[test]
fn a_module_binder_a_slot_s_signature_asks_more_of_refuses_the_load_uncalled() {
    let signatures = "SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
                      SIG Extra = #[(VAL extra :Str)]\n\
                      SIG Named = #[(VAL name :Str)]\n\
                      MODULE ints = ((LET Carrier = Number) (LET zero = 0) (LET name = \"i\"))\n";
    assert_eq!(
        run(&format!(
            "{signatures}LET wants = (FN :{{x :Extra}} -> Any = #(x.extra))\n\
             LET peek = (FN :{{m :Counter}} -> Any = #(wants {{x = m}}))\nPRINT \"start\""
        )),
        "load: <test>:6:40: this argument is :{x :SIG FOR ALL #{Carrier: Any} (zero: Carrier)}, \
         which can never satisfy the parameters :{x :SIG (extra: Str)} of :(FN :{x :SIG (extra: \
         Str)} -> Any)"
    );
    assert_eq!(
        run(&format!(
            "{signatures}LET peek = (FN :{{m :(Counter & Named)}} -> Any = #(m.extra))\n\
             PRINT \"start\""
        )),
        "load: <test>:5:50: SIG (name: Str) & SIG FOR ALL #{Carrier: Any} (zero: Carrier) has no \
         member extra"
    );
    assert_eq!(
        run(&format!(
            "{signatures}LET both = (FN :{{m :(Counter & Named)}} -> Any = #(m.name))\n\
             LET zeroed = (FN :{{m :(Counter & Named)}} -> Any = #(m.zero))\n\
             PRINT (both {{m = ints}})\nPRINT (zeroed {{m = ints}})"
        )),
        "i\n0"
    );
}

/// A call the text selects unfolds over a run-bound name, so what the callee builds refuses at
/// load; a call only a binding selects is left to the binding, which refuses at the build.
#[test]
fn a_call_over_a_name_unfolds_where_the_text_selects_it() {
    let built =
        |zero: &str| format!("(MODULE built OVER #[e] = (LET Zero = {zero}))\n    built\n  )\n");
    let subject = |inner: &str| {
        format!(
            "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  {inner}\
             LET a = (MAKE x)\n  LET z :(a.Zero) = \"no\"\n  z\n)\nPRINT \"start\"\n\
             PRINT (SUBJECT 5)"
        )
    };
    let refused = "this value is Str, which can never satisfy its annotation Number";
    let nested = subject(&format!(
        "EXPR #(MAKE e :Elt) -> Module = #(\n    {}  ",
        built("Number")
    ));
    assert_eq!(run(&nested), format!("load: <test>:7:3: {refused}"));
    let quantified = format!(
        "EXPR FOR ALL #[Tee] #(MAKE e :Tee) -> Module = #(\n  {}{}",
        built("Number").replace("\n    ", "\n  "),
        subject("")
    );
    assert_eq!(run(&quantified), format!("load: <test>:7:3: {refused}"));
    let overloads = subject(&format!(
        "EXPR #(BUILD e :Number) -> Module = #(\n    {}  EXPR #(BUILD e :Str) -> Module = #(\n    \
         {}  ",
        built("Number"),
        built("Str")
    ))
    .replace("(MAKE x)", "(BUILD x)");
    assert_eq!(
        run(&overloads),
        format!("load: <test>:11:3: {refused}\n  in (SUBJECT 5) at <test>:15:7")
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
    // A value the load cannot see is no module faults where the view door refuses it.
    assert_eq!(
        run(&format!(
            "{hide}SIG Shape = #[(VAL x :Number)]\nLET y = (HIDE 1)\nPRINT (y :| Shape)"
        )),
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
        display_name(
            typed.upper,
            &reading(program.tiers(), &Bump::new()),
            program.symbols(),
        )
        .to_string()
    });
    assert_eq!(run(&format!("{source}PRINT m")), at_load);
    // A member whose static type is at most another makes the binder at most `Module`.
    let inexact = "MODULE m = (LET x = (EVAL #(1) -> Any))";
    loaded(inexact, |program| {
        let typed = interval(program, program.shape(), "m");
        assert!(!typed.is_exact());
        assert_eq!(
            display_name(
                typed.upper,
                &reading(program.tiers(), &Bump::new()),
                program.symbols()
            )
            .to_string(),
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

/// `SIG Mapper`, whose quantified `apply` names its head parameter, and `MODULE nums`, which fits
/// it.
const MAPPER: &str = "SIG Mapper FOR ALL #[Carrier] = #[(VAL seed :Carrier) \
                      (VAL apply :(FN FOR ALL #[Elt] :{x :Carrier, y :Elt} -> Elt))]\n\
                      MODULE nums = ((LET seed = 1) \
                      (LET apply = (FN FOR ALL #[Elt] :{x :Number, y :Elt} -> Elt = #(y))))\n";

/// A quantified member over a head parameter its signature leaves unpinned is instantiated where
/// it is wanted, the parameter read as the path through its module: at a closed view's carrier,
/// and through a parameter at its path, so a plain module and a view behind its barrier both run.
/// An instance the wanted type can never fix still refuses the load, and a call's head still reads
/// the member as it is.
#[test]
fn a_quantified_member_over_an_unpinned_parameter_is_instantiated_at_its_path() {
    let wanted = "LET go :(FN :{x :(m.Carrier), y :Number} -> Number) = m.apply";
    assert_eq!(
        run(&format!(
            "{MAPPER}LET m = (nums :| Mapper)\n{wanted}\nPRINT (go {{x = m.seed, y = 4}})"
        )),
        "4"
    );
    assert_eq!(
        run(&format!(
            "{MAPPER}EXPR #(USEIT m :Mapper) -> Any = #(({wanted}) (go {{x = m.seed, y = 4}}))\n\
             PRINT (USEIT nums)\n\
             PRINT (USEIT (nums :| Mapper))"
        )),
        "4\n4"
    );
    assert_eq!(
        run(&format!(
            "{MAPPER}EXPR #(USEIT m :Mapper) -> Any = \
             #(LET go :(FN :{{x :(m.Carrier), y :Number}} -> Str) = m.apply)"
        )),
        "load: <test>:3:87: :(FN FOR ALL #[Elt] :{x :Carrier y :Elt} -> Elt) has no instance \
         under :(FN :{x :Never y :Number} -> Str)"
    );
    assert_eq!(
        run(&format!(
            "{MAPPER}EXPR #(USEIT m :Mapper) -> Any = #(m.apply {{x = m.seed, y = 2}})\n\
             PRINT (USEIT nums)"
        )),
        "2"
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
        "error: ambiguous call of `PICK _`: 2 overloads admit (Number) and none ranks first"
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
    // The load sees the view exactly, so it refuses an argument the view never sealed; through
    // `Any` the barrier refuses it where it runs.
    assert_eq!(
        run(&format!(
            "{COUNTER}LET v = (ints :| Counter)\nPRINT (v.succ {{x = 1}})"
        )),
        "load: <test>:4:7: this argument is :{x :Number}, which can never satisfy the parameters \
         :{x :Carrier} of :(FN :{x :Carrier} -> Carrier)",
        "an argument the view never sealed, at load"
    );
    assert_eq!(
        run(&format!(
            "{COUNTER}LET call = (FN :{{v :Any}} -> Any = #(v.succ {{x = 1}}))\n\
             PRINT (call {{v = (ints :| Counter)}})"
        )),
        "error: :(FN :{x :Carrier} -> Carrier) cannot be called with :{x :Number}: it is not \
         sealed under Carrier\n  in :(FN :{x :Carrier} -> Carrier) (v.succ {x = 1}) at <test>:3:36\n  in :(FN :{v :Any} -> Any) (call {v = (ints :| Counter)}) at <test>:4:7",
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
/// functions see its payload behind their barriers, and the view still fits its signature. A view
/// of a module the load sees all of is known at load, carrier and all, so a use of its member no
/// `Number` slot takes refuses the load; one through a parameter is refused where it runs, the
/// view seen at the parameter's signature.
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
    // The load lays `USE c`'s layer at `c`'s closed tree, which refuses the load, naming the call.
    let refused_plus = "load: <test>:4:34: no overload of `_ + _` admits these \
                        arguments:\n  Carrier, which can never satisfy Number\n  in (USE c) at \
                        <test>:10:7";
    let refused_take = "no overload of `TAKE _` admits this argument:\n  SIG (Carrier: Carrier, \
                        zero: Carrier, succ: :(FN :{x :Carrier} -> Carrier)), which can never \
                        satisfy SIG (zero: Number)";
    let loaded_take = format!("load: <test>:10:7: {refused_take}");
    // Through `p :Counter`, the view is seen at `Counter` pinned at its own carrier; the layer the
    // load lays for `PASS c` refuses the load, naming the call.
    let passed_take = "load: <test>:9:35: no overload of `TAKE _` admits this argument:\n  SIG FOR \
                    ALL #{Carrier: Number} (zero: Carrier, succ: :(FN :{x :Carrier} -> Carrier)) \
                    WITH {Carrier = Carrier}, which can never satisfy SIG (zero: Number)\n  in \
                    (PASS c) at <test>:10:7";
    for (program, printed) in [
        (
            "PRINT (c.zero + 1)",
            "load: <test>:10:7: no overload of `_ + _` admits these arguments:\n  Carrier, which \
             can never satisfy Number",
        ),
        (
            "PRINT (SHOW c.zero)",
            "load: <test>:10:7: no overload of `SHOW _` admits this argument:\n  Carrier, which can \
             never satisfy Number",
        ),
        (
            "LET n :Number = c.zero\nPRINT n",
            "load: <test>:10:1: this value is Carrier, which can never satisfy its annotation \
             Number",
        ),
        ("PRINT (c.zero == 4)", "false"),
        ("PRINT (c.succ {x = c.zero})", "Carrier(5)"),
        ("PRINT (USE ints)", "5"),
        ("PRINT (USE c)", refused_plus),
        ("PRINT (STEP c)", "Carrier(5)"),
        // A bounded parameter offers no `Number`, so neither the view nor a `Counter` fits `Plain`.
        ("PRINT (TAKE ints)", "5"),
        ("PRINT (TAKE c)", &loaded_take),
        ("PRINT (PASS c)", passed_take),
    ] {
        assert_eq!(run(&source(program)), printed, "{program}");
    }
}

/// An opaque view of an opaque view is built over its underlying: each member sealed once under its
/// own carrier, behind one barrier over the underlying's function, so a call crosses one barrier
/// each way; a value sealed under the inner view is no value of the outer.
#[test]
fn an_opaque_view_of_an_opaque_view_is_built_over_its_underlying() {
    let source = |program: &str| {
        format!(
            "SIG Counter FOR ALL #[Carrier] = \
             #[(VAL zero :Carrier) (VAL one :Carrier) (VAL succ :(FN :{{x :Carrier}} -> Carrier))]\n\
             SIG Again FOR ALL #[Carrier] = \
             #[(VAL zero :Carrier) (VAL succ :(FN :{{x :Carrier}} -> Carrier))]\n\
             MODULE ints = ((LET zero = 4) (LET one = 1) \
             (LET succ = (FN :{{x :Number}} -> Number = #(x + 1))))\n\
             LET c = (ints :| Counter)\nLET d = (c :| Again)\n{program}"
        )
    };
    assert_eq!(
        run(&source("PRINT (d.succ {x = (d.succ {x = d.zero})})")),
        "Carrier(6)"
    );
    assert_eq!(
        run(&source("PRINT (d.succ {x = c.zero})")),
        "load: <test>:6:7: this argument is :{x :Carrier}, which can never satisfy the parameters \
         :{x :Carrier} of :(FN :{x :Carrier} -> Carrier)"
    );
    assert_eq!(
        run(&source(
            "LET call = (FN :{e :Any, z :Any} -> Any = #(e.succ {x = z}))\n\
             PRINT (call {e = d, z = c.zero})"
        )),
        "error: :(FN :{x :Carrier} -> Carrier) cannot be called with :{x :Carrier}: it is not \
         sealed under Carrier\n  in :(FN :{x :Carrier} -> Carrier) (e.succ {x = z}) at <test>:6:44\n  in :(FN :{e :Any z :Any} -> Any) (call {e = d, z = c.zero}) at <test>:7:7"
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
        "load: <test>:4:7: this argument is :{x :Number}, which can never satisfy the parameters \
         :{x :(Carrier | Str)} of :(FN :{x :(Carrier | Str)} -> Str)"
    );
    assert_eq!(
        run(&format!(
            "{source}LET call = (FN :{{w :Any}} -> Any = #(w.show {{x = 1}}))\n\
             PRINT (call {{w = v}})"
        )),
        "error: :(FN :{x :(Carrier | Str)} -> Str) cannot be called with :{x :Number}: no member \
         of its union admits it\n  in :(FN :{x :(Carrier | Str)} -> Str) (w.show {x = 1}) at <test>:4:36\n  in :(FN :{w :Any} -> Any) (call {w = v}) at <test>:5:7"
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
             LET call = (FN :{{f :Any, z :Any}} -> Any = #(f {{x = z}}))\n\
             PRINT (a.Carrier == b.Carrier)\nPRINT (a.Carrier == c.Carrier)\n\
             PRINT (call {{f = a.succ, z = c.zero}})"
        )),
        "true\nfalse\nerror: :(FN :{x :Carrier} -> Carrier) cannot be called with \
         :{x :Carrier}: it is not sealed under Carrier\n  in :(FN :{x :Carrier} -> Carrier) (f {x = z}) at <test>:8:44\n  in :(FN :{f :Any z :Any} -> Any) (call {f = a.succ, z = c.zero}) at <test>:11:7",
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

#[test]
fn a_path_through_a_parameter_is_what_its_module_fits_the_declared_type_at() {
    let source = "SIG Mapper FOR ALL #[Carrier] = #[(VAL seed :Carrier)]\n\
                  LET carrier = (FN :{p :Mapper} -> Any = #(:(p.Carrier)))\n\
                  MODULE plain = (LET seed = 1)\n\
                  MODULE raw = ((LET Carrier = Str) (LET seed = 2))\n\
                  PRINT (carrier {p = (plain :| Mapper)})\n\
                  PRINT (carrier {p = plain})\n\
                  PRINT (carrier {p = raw})\n\
                  PRINT raw.Carrier";
    assert_eq!(
        run(source),
        "Carrier\nNumber\nNumber\nStr",
        "a view's carrier, the type a plain module realizes, and that type again where the \
         module binds `Carrier` to another, which a read through the module itself reads"
    );
}

/// Through a parameter declared at an application pinning the label, the path is the pin: at load,
/// which refuses a value off it, and at run, over a module that names no such type member.
#[test]
fn a_path_through_a_parameter_reads_the_pin_of_its_declared_application() {
    let program = |value: &str| {
        format!(
            "SIG Mapper FOR ALL #[Carrier] = #[(VAL seed :Carrier)]\n\
             MODULE plain = (LET seed = 1)\n\
             LET carrier = (FN :{{p :(Mapper WITH {{Carrier = Number}})}} -> Any = #(:(p.Carrier)))\n\
             PRINT (carrier {{p = plain}})\n\
             LET keep = (FN :{{p :(Mapper WITH {{Carrier = Number}})}} -> Any = \
             #((LET x :(p.Carrier) = {value}) (PRINT x)))\n\
             keep {{p = plain}}"
        )
    };
    assert_eq!(run(&program("2")), "Number\n2");
    assert_eq!(
        run(&program("\"s\"")),
        "load: <test>:5:66: this value is Str, which can never satisfy its annotation Number"
    );
}

/// A pin of one head parameter decides which of a module's overloads fits a keyworded member, and
/// so what another parameter solves to: the path reads through the declared application, pins and
/// all, even where a pin names a type the run binds and the reader names it nowhere.
#[test]
fn a_path_reads_through_every_pin_of_its_declared_application() {
    let source = "SIG Mapper FOR ALL #[Carrier Elt] = \
                  #[(VAL make :(FN :{n :Number} -> Carrier)) \
                  (EXPR #(STEP _ :Carrier _ :Elt) -> Carrier)]\n\
                  MODULE two = ((LET make = (FN :{n :Number} -> Number = #(n))) \
                  (EXPR #(STEP x :Number y :Str) -> Number = #(x)) \
                  (EXPR #(STEP x :(Number | Bool) y :Number) -> (Number | Bool) = #(x)))\n\
                  EXPR #(READER Elt :Type m :Any) -> Any = \
                  #((LET p :(Mapper WITH {Elt = Elt}) = m) ((FN :{} -> Any = #(:(p.Carrier))) {}))\n\
                  PRINT (READER Number two)\n\
                  PRINT (READER Str two)";
    assert_eq!(run(source), ":(Bool | Number)\nNumber");
}

/// A pin over a callable's own variable is solved where the argument fits the application: off a
/// module exact at its own type, a view's carrier, and an argument typed only at run.
#[test]
fn a_pin_over_a_callables_variable_solves_it() {
    let mapper = "SIG Mapper FOR ALL #[Carrier Elt] = \
                  #[(VAL make :(FN :{n :Number} -> Carrier)) \
                  (EXPR #(STEP _ :Carrier _ :Elt) -> Carrier)]\n\
                  MODULE two = ((LET make = (FN :{n :Number} -> Number = #(n))) \
                  (EXPR #(STEP x :Number y :Str) -> Number = #(x)) \
                  (EXPR #(STEP x :(Number | Bool) y :Number) -> (Number | Bool) = #(x)))\n\
                  EXPR FOR ALL #[Elt] #(USE x :Elt p :(Mapper WITH {Elt = Elt})) -> Any = #(Elt)\n\
                  PRINT (USE 1 two)";
    assert_eq!(run(mapper), "Number");
    let source = format!(
        "{ECHO}LET c = (ints :| Counter)\n\
         EXPR #(VIA m :Any) -> Any = #(ECHO m)\n\
         PRINT (ECHO ints)\nPRINT (ECHO c)\nPRINT (VIA ints)"
    );
    assert_eq!(run(&source), "0\nCarrier(0)\n0");
}

/// `Counter` over one member, `ECHO`, whose argument pins `Counter`'s carrier to its own variable,
/// and `ints`.
const ECHO: &str = "SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
                    EXPR FOR ALL #[Cr] #(ECHO elem :(Counter WITH {Carrier = Cr})) -> Cr = \
                    #(elem.zero)\n\
                    MODULE ints = ((LET zero = 0))\n";

/// A value the run retypes by its fit at a binder is exactly the application pinned at the paths
/// through it, so `ECHO` solves its carrier at load: a parameter, an annotated `LET`, a `LET`
/// binding a `:!`, a call or an `EVAL` whole, a member read through a name chain and an arm's
/// `it`.
#[test]
fn a_value_retyped_by_its_fit_at_a_binder_solves_a_pin_over_a_variable() {
    let source = format!(
        "{ECHO}SIG Outer = #[(VAL inner :Counter)]\n\
         MODULE outer = ((LET inner = ints))\n\
         EXPR #(HAS h :Counter) -> Any = #(ECHO h)\n\
         EXPR #(NOTED g :Any) -> Any = #((LET a :Counter = g) (ECHO a))\n\
         EXPR #(HELD g :Any) -> Any = #((LET m = (g :! Counter)) (ECHO m))\n\
         EXPR #(INNER o :Outer) -> Any = #(ECHO o.inner)\n\
         EXPR #(ARM g :Any) -> Any = #(MATCH g WITH #{{Counter: (ECHO it), _: (7)}})\n\
         EXPR #(MAKE g :Any) -> Counter = #(g)\n\
         EXPR #(CALLED g :Any) -> Any = #((LET n = (MAKE g)) (ECHO n))\n\
         EXPR #(EVALED g :Any) -> Any = #((LET e = (EVAL #($g) -> Counter)) (ECHO e))\n\
         PRINT (HAS ints)\nPRINT (NOTED ints)\nPRINT (HELD ints)\nPRINT (INNER outer)\n\
         PRINT (ARM ints)\nPRINT (ARM 1)\nPRINT (CALLED ints)\nPRINT (EVALED ints)"
    );
    assert_eq!(run(&source), "0\n0\n0\n0\n0\n7\n0\n0");
}

/// A binder pinned in its own unit is read from a callable born after it, at every call of the
/// body: each walk reaches its path's class alike.
#[test]
fn a_binder_pinned_in_its_own_unit_is_read_by_a_later_callable() {
    let source = format!(
        "{ECHO}EXPR #(HELD g :Any) -> Any = \
         #((LET m = (g :! Counter)) (LET f = (FN :{{}} -> Any = #(ECHO m))) (f {{}}))\n\
         EXPR #(TWICE g :Any) -> Any = #((PRINT (HELD g)) (HELD g))\n\
         PRINT (TWICE ints)"
    );
    assert_eq!(run(&source), "0\n0");
}

/// The run reads a path through module members alone, so a read through a record's field stays
/// at most its signature: refused at load, through a record parameter or a record holding a value
/// retyped in place.
#[test]
fn a_field_read_stays_at_most_its_signature() {
    for go in [
        "EXPR #(GO rr :{c :Counter}) -> Any = #(ECHO rr.c)\nPRINT (GO {c = ints})",
        "EXPR #(GO g :Any) -> Any = #((LET r = {c = (g :! Counter)}) (ECHO r.c))\nPRINT (GO ints)",
    ] {
        let refused = run(&format!("{ECHO}{go}"));
        assert!(refused.contains("no overload of `ECHO _`"), "{refused}");
    }
}

/// A value retyped in place has no binder to read its paths through: it stays at most its
/// signature, which can never fit a pin over a variable.
#[test]
fn a_value_retyped_in_place_stays_at_most_its_signature() {
    let source = format!("{ECHO}EXPR #(INLINE g :Any) -> Any = #(ECHO (g :! Counter))");
    let refused = run(&source);
    assert!(refused.contains("no overload of `ECHO _`"), "{refused}");
}

/// A body holding a value no module binder names in place, beside a binder that names it, is
/// refused at the load as the hold alone is: the binder is no unknown the run binds behind the
/// hold's back.
#[test]
fn a_hold_in_place_beside_a_binder_of_its_operand_is_refused_at_the_load() {
    for go in [
        "EXPR #(MIX g :Any) -> Any = \
         #((LET x = g) (PRINT (ECHO (g :! Counter))) (ECHO (x :! Counter)))\nPRINT (MIX ints)",
        "EXPR #(BOTH g :Any) -> Any = \
         #((LET m = (g :! Counter)) (PRINT (ECHO m)) (ECHO (g :! Counter)))\nPRINT (BOTH ints)",
    ] {
        let refused = run(&format!("{ECHO}{go}"));
        assert!(refused.contains("no overload of `ECHO _`"), "{refused}");
    }
}

/// A pin over a variable solves it once across the call: two arguments that pin it to unrelated
/// types refuse the call, and one that agrees solves it.
#[test]
fn a_pin_and_a_slot_solve_one_variable_jointly() {
    let both = |argument: &str| {
        format!(
            "{ECHO}EXPR #(BOTH 1 WITH 1)\n\
             EXPR FOR ALL #[Cr] #(BOTH elem :(Counter WITH {{Carrier = Cr}}) WITH x :Cr) -> Any \
             = #(Cr)\n\
             PRINT (BOTH ints WITH {argument})"
        )
    };
    assert_eq!(run(&both("1")), "Number");
    let refused = run(&both("\"s\""));
    assert!(
        refused.contains("no overload of `BOTH _ WITH _`"),
        "{refused}"
    );
}

/// A family parameter inside a pin is refused, as one at a contravariant position is: a pin is
/// invariant.
#[test]
fn a_family_parameter_inside_a_pin_refuses_the_load() {
    let refused = run("SIG Stack FOR ALL #[Elt] = #[(VAL top :Elt)]\n\
                       UNION (Elem -> Holder) = #{Has: :(Stack WITH {Elt = Elem}), Not: Null}");
    assert!(refused.contains("<test>:2:"), "{refused}");
}

/// `Counter`, two signatures each naming one of its members, `Shown` showing `zero` at `Any`, and
/// `c`, a view of `ints` at `Counter`: the views of a view the tests below take.
const VIEWED: &str = "SIG Counter FOR ALL #[Carrier] = \
                      #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                      SIG Counter0 FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
                      SIG Stepper FOR ALL #[Carrier] = #[(VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                      SIG Shown = #[(VAL zero :Any)]\n\
                      MODULE ints = ((LET zero = 4) (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n\
                      LET c = (ints :| Counter)\n";

/// A view of a view at its own application is that view: its carrier is the source's.
#[test]
fn a_view_of_a_view_at_its_own_exposure_is_the_view() {
    let source = format!("{VIEWED}LET k :(c.Carrier) = (c :| Counter).zero\nPRINT k");
    assert_eq!(run(&source), "Carrier(4)");
}

/// A view of a view is built over its underlying: `(c :| Counter0)` and `(ints :| Counter0)` are
/// one module, so a value either seals passes a slot typed at the other's carrier.
#[test]
fn a_view_of_a_view_is_the_view_of_its_underlying() {
    let source = format!(
        "{VIEWED}LET a = (c :| Counter0)\nLET b = (ints :| Counter0)\n\
         LET f = (FN :{{x :(b.Carrier)}} -> Any = #(x))\n\
         LET g = (FN :{{x :(a.Carrier)}} -> Any = #(x))\n\
         PRINT (f {{x = a.zero}})\nPRINT (g {{x = b.zero}})"
    );
    assert_eq!(run(&source), "Carrier(4)\nCarrier(4)");
}

/// *Fits* admits a view of a view over its source as written: `Plain` fits `ints` but not `c`,
/// whose `zero` is sealed.
#[test]
fn a_view_of_a_view_is_admitted_over_its_source() {
    let plain = "SIG Plain = #[(VAL zero :Number)]\n";
    assert_eq!(
        run(&format!("{VIEWED}{plain}PRINT (ints :| Plain).zero")),
        "4"
    );
    assert_eq!(
        run(&format!("{VIEWED}{plain}LET p = (c :| Plain)")),
        "load: <test>:8:9: this value is SIG (Carrier: Carrier, zero: Carrier, succ: :(FN :{x \
         :Carrier} -> Carrier)), which can never satisfy its ascription SIG (zero: Number)"
    );
}

/// A header a view's source is seen through only admits the view: a widening is forgotten once
/// the view hides the parameter again, over a module or over a view of it.
#[test]
fn a_header_under_a_view_is_forgotten() {
    let ord =
        "SIG Ord FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\nMODULE nums = ((LET zero = 4))\n";
    let over_module = format!(
        "{ord}LET plain = (nums :| Ord)\n\
         LET wide = ((nums :! (Ord WITH {{Carrier = Any}})) :| Ord)\n\
         LET take = (FN :{{x :(plain.Carrier)}} -> Any = #(x))\n\
         PRINT (take {{x = wide.zero}})"
    );
    assert_eq!(run(&over_module), "Carrier(4)");
    let over_view = format!(
        "{ord}LET c0 = (nums :| Ord)\n\
         LET again = ((c0 :! (Ord WITH {{Carrier = Any}})) :| Ord)\n\
         LET take = (FN :{{x :(c0.Carrier)}} -> Any = #(x))\n\
         PRINT (take {{x = again.zero}})\nPRINT (again.Carrier == c0.Carrier)"
    );
    assert_eq!(run(&over_view), "Carrier(4)\ntrue");
}

/// What one view shows at `Any` and the next hides again is forgotten: `((nums :| Wide) :| Took)`
/// is `(nums :| Took)`.
#[test]
fn a_widening_through_a_view_is_forgotten_once_hidden_again() {
    let source = "SIG Wide FOR ALL #[Carrier] = #[(VAL zero :Carrier) (VAL x :Any)]\n\
                  SIG Took FOR ALL #[Elem] = #[(VAL x :Elem)]\n\
                  MODULE nums = ((LET zero = 4) (LET x = 7))\n\
                  LET a = ((nums :| Wide) :| Took)\nLET b = (nums :| Took)\n\
                  LET take = (FN :{x :(b.Elem)} -> Any = #(x))\n\
                  PRINT (a.Elem == b.Elem)\nPRINT (take {x = a.x})";
    assert_eq!(run(source), "true\nElem(7)");
}

/// What *fits* solves over the underlying names no carrier of the source: a parameter hiding a
/// whole list the source sealed cell by cell holds the underlying's bare list.
#[test]
fn a_view_of_a_view_solves_over_the_underlying() {
    let source = "SIG Listed FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL pair :(LIST OF Carrier))]\n\
                  SIG Paired FOR ALL #[Elem] = #[(VAL pair :Elem)]\n\
                  MODULE nums = ((LET zero = 4) (LET pair = [1 2]))\n\
                  LET v = (nums :| Listed)\n\
                  PRINT (v :| Paired).pair\n\
                  PRINT ((v :| Paired).Elem == (nums :| Paired).Elem)";
    assert_eq!(run(source), "Elem([1, 2])\ntrue");
}

/// A signature naming the source's carrier, ascribed or held by a pin, is read back onto the
/// underlying, inside the signature too: what is written over a module at a type is written over
/// its view at the carrier hiding that type.
#[test]
fn a_view_of_a_view_reads_back_a_carrier_inside_a_signature() {
    let ascribed = "SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
                    MODULE ints = ((LET zero = 0))\n\
                    LET c = (ints :| Counter)\n\
                    LET Hid = c.Carrier\n\
                    SIG Named = #[(VAL zero :Hid)]\n\
                    LET d = (c :| Named)\n\
                    PRINT d.zero";
    assert_eq!(run(ascribed), "Carrier(0)");
    let pinned = "SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
                  SIG Outer FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL m :(Counter WITH {Carrier = Carrier}))]\n\
                  MODULE ints = ((LET zero = 0) (MODULE m = ((LET zero = 0))))\n\
                  LET c = (ints :| Outer)\n\
                  LET Hid = c.Carrier\n\
                  SIG Named = #[(VAL zero :Hid)]\n\
                  SIG Wrap FOR ALL #[Elem] = #[(VAL m :Elem)]\n\
                  LET d = (c :| (Wrap WITH {Elem = Named}))\n\
                  PRINT d.m.zero";
    assert_eq!(run(pinned), "Carrier(0)");
    // Inward too: a module handed to a function member at such a signature is re-viewed at it
    // read back, its member unsealed for the underlying function.
    let inward = "SIG Zeroed = #[(VAL zero :Number)]\n\
                  SIG Inner FOR ALL #[Elem] = #[(VAL zero :Elem)]\n\
                  SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier) \
                  (VAL take :(FN :{m :(Inner WITH {Elem = Carrier})} -> Number))]\n\
                  MODULE ints = ((LET zero = 0) \
                  (LET take = (FN :{m :Zeroed} -> Number = #(m.zero + 1))))\n\
                  LET c = (ints :| Counter)\n\
                  LET Hid = c.Carrier\n\
                  SIG Named = #[(VAL zero :Hid)]\n\
                  SIG Taker = #[(VAL take :(FN :{m :Named} -> Number))]\n\
                  LET d = (c :| Taker)\n\
                  PRINT (d.take {m = c})";
    assert_eq!(run(inward), "1");
}

/// A `FOR ALL` bound may be a signature naming a view's carrier, or name a recursive group's
/// fellow inside a signature: either is a type variable's bound like any other, and the seal
/// reads the fellow in the bound as the member it names.
#[test]
fn a_bound_may_hold_a_carrier_or_a_fellow_inside_a_signature() {
    let carrier = "SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
                   MODULE ints = ((LET zero = 0))\n\
                   LET c = (ints :| Counter)\n\
                   LET Hid = c.Carrier\n\
                   SIG Named = #[(VAL zero :Hid)]\n\
                   EXPR FOR ALL #{Mod: Named} #(PICK m :Mod) -> Number = #(1)\n\
                   PRINT (PICK c)";
    assert_eq!(run(carrier), "1");
    let fellow = "SIG Shelf = #[(VAL pick :(FN FOR ALL #{Elem: Leaf} :{x :Elem} -> Number)) \
                  (VAL kid :Leaf)]\n\
                  NEWTYPE Leaf = :{shelf :Shelf}\n\
                  PRINT Shelf";
    // The sealed signature names the fellow in the bound, as in the slot: no sibling is left.
    assert_eq!(
        run(fellow),
        "SIG (pick: :(FN FOR ALL #{Elem: Leaf} :{x :Elem} -> Number), kid: Leaf)"
    );
}

/// A view of a view that shows a part its source hides is refused, at load where the load sees
/// the source and at run otherwise, naming the member; over the module itself it is a view.
#[test]
fn a_view_of_a_view_showing_what_its_source_hides_is_refused() {
    assert_eq!(
        run(&format!("{VIEWED}LET s = (c :| Shown)")),
        "load: <test>:7:9: this view shows `zero` where its source hides it: a view of a view \
         must hide again everything its source hides"
    );
    assert_eq!(
        run(&format!(
            "{VIEWED}LET f = (FN :{{x :Any}} -> Any = #(x :| Shown))\nPRINT (f {{x = c}}).zero"
        )),
        "error: this view shows `zero` where its source hides it: a view of a view must hide \
         again everything its source hides\n  in :(FN :{x :Any} -> Any) (f {x = c}) at <test>:8:7"
    );
    assert_eq!(run(&format!("{VIEWED}PRINT (ints :| Shown).zero")), "4");
    let swapped = "SIG Shows FOR ALL #[Carrier] = #[(VAL a :Any) (VAL b :Carrier)]\n\
                   SIG Hides FOR ALL #[Carrier] = #[(VAL a :Carrier) (VAL b :Any)]\n\
                   MODULE m = ((LET a = 1) (LET b = 2))\n\
                   LET s0 = (m :| Shows)\nLET h = (s0 :| Hides)";
    assert_eq!(
        run(swapped),
        "load: <test>:5:9: this view shows `b` where its source hides it: a view of a view must \
         hide again everything its source hides"
    );
}

/// A sealed part inside a union is hidden again by the union member reading as its type, whatever
/// order the members are written in.
#[test]
fn a_union_member_hides_again_whatever_its_order() {
    let source = "SIG Either FOR ALL #[Carrier] = \
                  #[(VAL zero :(Carrier | Str)) (VAL one :Number)]\n\
                  SIG Either2 FOR ALL #[Carrier] = #[(VAL zero :(Str | Carrier))]\n\
                  MODULE nums = ((LET zero = 4) (LET one = 1))\n\
                  LET e = (nums :| Either)\n\
                  PRINT ((e :| Either).Carrier == e.Carrier)\n\
                  LET e2 = (e :| Either2)\nLET n2 = (nums :| Either2)\n\
                  LET take = (FN :{x :(n2.Carrier | Str)} -> Any = #(x))\n\
                  PRINT (take {x = e2.zero})\nPRINT (e2.Carrier == n2.Carrier)";
    assert_eq!(run(source), "true\nCarrier(4)\ntrue");
}

/// `Inner` with two parameters, an `Outer` declaring it nested over its own, and `v`, a view of a
/// module holding `parts` at it.
const NESTED: &str = "SIG Inner FOR ALL #[Elem Extra] = #[(VAL one :Elem) (VAL two :Extra)]\n\
                      SIG Outer FOR ALL #[Carrier] = #[(VAL zero :Carrier) \
                      (VAL inner :(Inner WITH {Elem = Carrier})) \
                      (VAL first :(FN :{x :(Inner WITH {Elem = Carrier})} -> Carrier))]\n\
                      MODULE parts = ((LET one = 1) (LET two = \"s\"))\n\
                      MODULE m = ((LET zero = 0) (LET inner = parts) \
                      (LET first = (FN :{x :(Inner WITH {Elem = Number})} -> Number = #(x.one))))\n\
                      LET v = (m :| Outer)\n";

/// A view's nested module member is an opaque view of the underlying's member: it hides the
/// nested signature's parameter its slot leaves unpinned, a view of it at its own application is
/// it, and a view of the outer view views it over the underlying's member again.
#[test]
fn a_nested_module_member_is_an_opaque_view() {
    assert_eq!(run(&format!("{NESTED}PRINT v.inner.two")), "Extra(s)");
    let again = format!(
        "{NESTED}LET i = v.inner\nLET w = (i :| (Inner WITH {{Elem = v.Carrier}}))\n\
         LET take = (FN :{{x :(i.Extra)}} -> Any = #(x))\n\
         PRINT (w.Extra == i.Extra)\nPRINT (take {{x = w.two}})"
    );
    assert_eq!(run(&again), "true\nExtra(s)");
    let both = format!(
        "{NESTED}SIG OuterInner FOR ALL #[Carrier] = #[(VAL inner :(Inner WITH {{Elem = Carrier}}))]\n\
         LET a = (v :| OuterInner).inner\nLET b = (m :| OuterInner).inner\n\
         PRINT (a.Elem == b.Elem)\nPRINT (a.Extra == b.Extra)"
    );
    assert_eq!(run(&both), "true\ntrue");
    let whole = format!(
        "{NESTED}SIG Whole FOR ALL #[Part] = #[(VAL inner :Part)]\n\
         PRINT ((v :| Whole).Part == (m :| Whole).Part)\n\
         PRINT (v :| Whole).inner\nPRINT (m :| Whole).inner"
    );
    assert_eq!(
        run(&whole),
        "true\nPart(SIG (two: Str, one: Number))\nPart(SIG (two: Str, one: Number))"
    );
    let shown = format!(
        "{NESTED}SIG OuterShow FOR ALL #[Carrier] = \
         #[(VAL inner :(Inner WITH {{Elem = Carrier, Extra = Any}}))]\nLET r = (v :| OuterShow)"
    );
    assert_eq!(
        run(&shown),
        "load: <test>:7:9: this view shows `inner` where its source hides it: a view of a view \
         must hide again everything its source hides"
    );
    assert_eq!(
        run(&format!("{NESTED}PRINT (v.first {{x = v.inner}})")),
        "Carrier(1)"
    );
}

/// A nested module a call through a barrier passes inward is re-viewed transparently, and a view of
/// it inside the function is built over it: its own underlying.
#[test]
fn an_inward_re_view_is_its_own_underlying() {
    let source = "SIG Inner FOR ALL #[Elem Extra] = #[(VAL one :Elem) (VAL two :Extra)]\n\
                  SIG Outer FOR ALL #[Carrier] = #[(VAL zero :Carrier) \
                  (VAL inner :(Inner WITH {Elem = Carrier})) \
                  (VAL first :(FN :{x :(Inner WITH {Elem = Carrier})} -> Any)) \
                  (VAL pinned :(FN :{x :(Inner WITH {Elem = Carrier})} -> Any))]\n\
                  MODULE parts = ((LET one = 1) (LET two = \"s\"))\n\
                  MODULE m = ((LET zero = 0) (LET inner = parts) \
                  (LET first = (FN :{x :(Inner WITH {Elem = Number})} -> Any = \
                  #((x :| Inner).one))) \
                  (LET pinned = (FN :{x :(Inner WITH {Elem = Number})} -> Any = \
                  #((x :| (Inner WITH {Elem = Number})).one))))\n\
                  LET v = (m :| Outer)\n";
    assert_eq!(
        run(&format!(
            "{source}PRINT (v.first {{x = v.inner}})\nPRINT (v.pinned {{x = v.inner}})"
        )),
        "Elem(1)\n1"
    );
    // A module built from a value the view sealed crosses in unsealed.
    assert_eq!(
        run(&format!(
            "{source}MODULE mine = ((LET one = v.zero) (LET two = 5))\nPRINT (v.first {{x = mine}})"
        )),
        "Elem(0)"
    );
}

/// A module a function member returns through its view's barrier is an opaque view at the return
/// type read on the view's side, hiding the parameters it leaves unpinned.
#[test]
fn a_module_a_barrier_returns_is_an_opaque_view() {
    let source = "SIG Inner FOR ALL #[Elem Extra] = #[(VAL one :Elem) (VAL two :Extra)]\n\
                  SIG Maker FOR ALL #[Carrier] = #[(VAL zero :Carrier) \
                  (VAL make :(FN :{x :Carrier} -> (Inner WITH {Elem = Carrier})))]\n\
                  MODULE ints = ((LET zero = 4) (LET make = (FN :{x :Number} -> \
                  (Inner WITH {Elem = Number, Extra = Str}) = \
                  #(MODULE made OVER #[x] = ((LET one = x) (LET two = \"a\"))))))\n\
                  LET v = (ints :| Maker)\nLET r = (v.make {x = v.zero})\n\
                  PRINT r.one\nPRINT r.two";
    assert_eq!(run(source), "Carrier(4)\nExtra(a)");
}

/// `Listed` sealing a list cell by cell, `Paired` naming its list, and `v`, a view of `nums` at
/// `Listed`: the pins over `v`'s carrier below.
const LISTED: &str = "SIG Listed FOR ALL #[Carrier] = \
                      #[(VAL zero :Carrier) (VAL pair :(LIST OF Carrier))]\n\
                      SIG Paired FOR ALL #[Elem] = #[(VAL pair :Elem)]\n\
                      SIG Zeroed FOR ALL #[Elem] = #[(VAL zero :Elem)]\n\
                      MODULE nums = ((LET zero = 4) (LET pair = [1 2]))\n\
                      LET v = (nums :| Listed)\n";

/// A pin over the source's carrier reads back onto the underlying, however deep in the pin the
/// carrier sits: what is written over `nums` with `Number` is written over `v` with `v.Carrier`.
/// The view seals each cell under `v`'s carrier, a view of it at its own application is it, one
/// hiding the list whole is `nums`'s, and one showing it at `Any` is refused.
#[test]
fn a_pin_over_a_sources_carrier_reads_back_onto_the_underlying() {
    let w = "LET w = (v :| (Paired WITH {Elem = (LIST OF v.Carrier)}))\n";
    assert_eq!(
        run(&format!(
            "{LISTED}{w}LET w2 = (w :| (Paired WITH {{Elem = (LIST OF v.Carrier)}}))\n\
             LET take = (FN :{{x :(LIST OF v.Carrier)}} -> Any = #(x))\n\
             PRINT (take {{x = w2.pair}})\nPRINT (take {{x = v.pair}})"
        )),
        "[Carrier(1), Carrier(2)]\n[Carrier(1), Carrier(2)]"
    );
    assert_eq!(
        run(&format!(
            "{LISTED}{w}LET h = (w :| Paired)\nLET g = (nums :| Paired)\n\
             PRINT (h.Elem == g.Elem)\nPRINT h.pair"
        )),
        "true\nElem([1, 2])"
    );
    assert_eq!(
        run(&format!(
            "{LISTED}LET f = (FN :{{x :Any}} -> Any = \
             #((x :| (Paired WITH {{Elem = (LIST OF v.Carrier)}})).pair))\nPRINT (f {{x = v}})"
        )),
        "[Carrier(1), Carrier(2)]"
    );
    assert_eq!(
        run(&format!(
            "{LISTED}{w}LET h = (w :| (Paired WITH {{Elem = Any}}))"
        )),
        "load: <test>:7:9: this view shows `pair` where its source hides it: a view of a view \
         must hide again everything its source hides"
    );
}

/// A pin at a union holding the source's carrier hides the sealed part at the member it reads as,
/// and a view hiding it again is the underlying's.
#[test]
fn a_union_pin_over_a_sources_carrier_hides_it_at_its_member() {
    let source = format!(
        "{LISTED}LET w = (v :| (Zeroed WITH {{Elem = (v.Carrier | Str)}}))\nPRINT w.zero\n\
         LET z = (w :| Zeroed)\nLET n = (nums :| Zeroed)\nPRINT (z.Elem == n.Elem)"
    );
    assert_eq!(run(&source), "Carrier(4)\ntrue");
}

/// A function-typed pin over the source's carrier is a barrier over the underlying's function: a
/// call crosses it inward through the carrier and back.
#[test]
fn a_function_pin_over_a_sources_carrier_crosses_one_barrier() {
    let source = "SIG Stepper FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                  SIG Fun FOR ALL #[Elem] = #[(VAL succ :Elem)]\n\
                  MODULE ints = ((LET zero = 4) (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n\
                  LET c = (ints :| Stepper)\n\
                  LET w = (c :| (Fun WITH {Elem = (FN :{x :(c.Carrier)} -> (c.Carrier))}))\n\
                  PRINT (w.succ {x = c.zero})";
    assert_eq!(run(source), "Carrier(5)");
}

/// A nested slot pinned at a structure over the enclosing carrier views the nested module over it,
/// and a view of it at its own application is it.
#[test]
fn a_nested_pin_over_the_enclosing_carrier_reads_back() {
    let source = "SIG Inner FOR ALL #[Elem Extra] = #[(VAL one :Elem) (VAL two :Extra)]\n\
                  SIG Outer FOR ALL #[Carrier] = #[(VAL zero :Carrier) \
                  (VAL inner :(Inner WITH {Elem = (LIST OF Carrier)}))]\n\
                  MODULE parts = ((LET one = [1 2]) (LET two = \"s\"))\n\
                  MODULE m = ((LET zero = 0) (LET inner = parts))\n\
                  LET v = (m :| Outer)\nPRINT v.inner.one\nLET i = v.inner\n\
                  LET j = (i :| (Inner WITH {Elem = (LIST OF v.Carrier)}))\n\
                  PRINT (j.Extra == i.Extra)";
    assert_eq!(run(source), "[Carrier(1), Carrier(2)]\ntrue");
}

/// A nested slot pinned at a union holding the enclosing carrier views the nested module's member
/// at the union's member over the carrier.
#[test]
fn a_nested_union_pin_over_the_enclosing_carrier_seals_at_its_member() {
    let source = "SIG Inner FOR ALL #[Elem] = #[(VAL one :Elem)]\n\
                  SIG Outer FOR ALL #[Carrier] = #[(VAL zero :Carrier) \
                  (VAL inner :(Inner WITH {Elem = (Carrier | Str)}))]\n\
                  MODULE parts = ((LET one = 1))\n\
                  MODULE m = ((LET zero = 0) (LET inner = parts))\n\
                  LET v = (m :| Outer)\n\
                  LET take = (FN :{x :(v.Carrier | Str)} -> Any = #(x))\n\
                  PRINT (take {x = v.inner.one})";
    assert_eq!(run(source), "Carrier(1)");
}

/// `SIG Node`, a counter module and a link to the next node, and `UNION Link` over it: a list of
/// modules a signature and a union declare together, and `WALK`, which sums the counters' carriers
/// along it.
const NODES: &str = "SIG Node = #[(VAL count :Counter) (VAL next :Link)]\n\
                     UNION Link = #{More: Node, End: Null}\n\
                     EXPR #(CARRIER m :Counter) -> Number = #((LET v = (m :| Counter)) (7))\n\
                     EXPR #(WALK n :Node) -> Number = \
                     #((CARRIER n.count) + (MATCH n.next UNDER Link WITH \
                     #{More: (WALK it), End: (0)}))\n\
                     MODULE last = ((LET count = ints) (LET next = (Link.End null)))\n\
                     MODULE first = ((LET count = ints) (LET next = (Link.More last)))\n";

/// A signature a nominal member names, and that names it back, declares a recursive structure of
/// modules: a module admits at it, and *fits* over it stops at the nominal.
#[test]
fn a_signature_in_a_recursive_group_types_a_list_of_modules() {
    assert_eq!(run(&format!("{COUNTER}{NODES}PRINT (WALK first)")), "14");
    assert_eq!(
        run(&format!(
            "{COUNTER}{NODES}LET n :Node = first\nPRINT (WALK n)"
        )),
        "14"
    );
    assert_eq!(
        run("SIG Holder = #[(VAL n :Nat)]\nNEWTYPE Nat = :{s :Holder}\nPRINT 1"),
        "1"
    );
    // A ring through signatures alone has no nominal to mint its identity.
    assert_eq!(
        run("SIG Chain = #[(VAL next :Chain)]\nPRINT 1"),
        "load: <test>:1:1: `Chain` names itself through signatures and aliases alone, so its type \
         is infinite; a recursive type passes through a NEWTYPE or a UNION"
    );
    // A payload that misses the signature is refused, as any payload that misses is.
    assert_eq!(
        run(&format!(
            "{COUNTER}{NODES}MODULE bad = ((LET count = ints) (LET next = 5))\n\
             LET wrong = (Link.More bad)\nPRINT \"built\""
        )),
        "error: More cannot wrap SIG (count: SIG (zero: Number, succ: :(FN :{x :Number} -> \
         Number)), next: Number): its representation is SIG (count: SIG FOR ALL #{Carrier: Any} \
         (zero: Carrier, succ: :(FN :{x :Carrier} -> Carrier)), next: :(End | More))"
    );
}

/// A value member read through a variable bounded by a signature declaring it lies under the
/// bound's member at load, so an annotation that member can never satisfy refuses the load, and
/// one it satisfies runs.
#[test]
fn a_member_read_through_a_signature_bound_lies_under_the_bound_s_member() {
    let has = "SIG Has = #[(VAL f :Number)]\nMODULE hm = ((LET f = 1))\nLET h :Has = hm\n";
    let program = |body: &str| {
        format!(
            "{has}EXPR FOR ALL #{{Elt: Has}} #(GET x :Elt) -> Any = #(\n  {body}\n)\nPRINT (GET h)"
        )
    };
    assert_eq!(
        run(&program("LET z :Str = x.f\n  z")),
        "load: <test>:5:3: this value is Number, which can never satisfy its annotation Str"
    );
    assert_eq!(run(&program("LET z = x.f\n  z")), "1");
}
