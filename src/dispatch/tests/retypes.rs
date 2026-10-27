//! The retype law: a value retyped to a type lies within the static type the load gives it there,
//! at every retype site. Each drawn type and value under it is rendered once per site, after the
//! nominal types the draw may name ([`NOMINALS`]).
//!
//! The oracle is the evaluator's debug net, which checks that every evaluated node's value carries
//! a type within its static type; a release build checks only that each site runs without a fault.
//!
//! A quote's code and a type value retype at a code kind and a kind as a value does at its type:
//! [`code_or_type`] draws one beside a kind above it, and the unit tests below check that each
//! dispatches at the kind its slot declares.

use proptest::prelude::*;

use super::generate::{Desc, NOMINALS, ascribed, retyped_type, spelled, value_under};
use super::run;

/// A type a retype re-stamps values at, beside a value under it.
fn retyped() -> impl Strategy<Value = (Desc, String)> {
    retyped_type().prop_flat_map(|desc| {
        let value = value_under(&desc);
        (Just(desc), value)
    })
}

/// One program per retype site, each retyping `value` to `desc` and ending in an evaluated node
/// whose static type is the retype's: a bare name is read in place and never checked, so each is
/// wrapped in a list.
fn sites(desc: &Desc, value: &str) -> [String; 5] {
    sites_at(&spelled(desc, &[]), &ascribed(desc, &[]), value)
}

/// [`sites`] at the type written `spelled`, and `ascribed` after a name it types.
fn sites_at(spelled: &str, ascribed: &str, value: &str) -> [String; 5] {
    [
        format!("PRINT [({value} :! {spelled})]"),
        format!("LET x {ascribed} = {value}\nPRINT [x]"),
        format!("LET f = (FN :{{x {ascribed}}} -> Any = #([x]))\nPRINT (f {{x = {value}}})"),
        // A quote's code reads a name from outside only through `$`.
        format!("LET v = {value}\nPRINT [(EVAL #($v) -> {spelled})]"),
        format!("EXPR #(GIVE x :Any) -> {spelled} = #({value})\nPRINT [(GIVE 1)]"),
    ]
    .map(|site| format!("{NOMINALS}\n{site}"))
}

/// A signature, so a type value of kind `Signature` can be drawn.
const SIGNED: &str = "SIG Signed = #[(VAL z :Number)]\n";

/// A quote's code or a type value, beside a type above it a retype may re-stamp it at, spelled as
/// a slot writes it: each code kind above the quote's own that has a name, needing a name or not;
/// `Type`, or `Signature` for a signature; and `Code`, `Any`, which re-stamp nothing.
fn code_or_type() -> impl Strategy<Value = (String, String)> {
    let quotes: [(&str, &[&str]); 6] = [
        ("#(y)", &["Name", "Symbol", "Expression", "Block"]),
        ("#(+)", &["Keyword", "Symbol", "Expression", "Block"]),
        ("#(42)", &["Literal", "Expression", "Block"]),
        ("#(y + 1)", &["Expression", "Block"]),
        (
            "#(LET z = 1)",
            &["Binder", "Declaration", "Expression", "Block"],
        ),
        ("#(:{x :Number})", &["Expression", "Block"]),
    ];
    let types: [(&str, &[&str]); 4] = [
        ("Number", &["Type"]),
        ("Boxed", &["Type"]),
        ("Maybe", &["Type"]),
        ("Signed", &["Type", "Signature"]),
    ];
    let quoted = (0..quotes.len(), any::<prop::sample::Index>(), any::<bool>()).prop_map(
        move |(at, kind, needing)| {
            let (value, kinds) = quotes[at];
            let kind = kind.get(kinds);
            let spelled = match needing {
                true => format!(":({kind} NEEDING #[w])"),
                false => kind.to_string(),
            };
            (value.to_string(), spelled)
        },
    );
    let typed = (0..types.len(), any::<prop::sample::Index>()).prop_map(move |(at, kind)| {
        let (value, kinds) = types[at];
        (value.to_string(), kind.get(kinds).to_string())
    });
    let tops = (
        prop_oneof![Just("#(y)"), Just("Number")],
        prop_oneof![Just("Code"), Just("Any")],
    )
        .prop_filter("a type value is no code", |(value, top)| {
            !(*value == "Number" && *top == "Code")
        })
        .prop_map(|(value, top)| (value.to_string(), top.to_string()));
    prop_oneof![4 => quoted, 2 => typed, 1 => tops]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: crate::tests::case_share(1, 4),
        ..ProptestConfig::default()
    })]

    /// A value retyped to a type lies within the static type the load gives it there, at every
    /// retype site: the evaluator's `carried_under_static` net checks each evaluated node, and the
    /// program must print without a fault.
    #[test]
    fn a_retyped_value_lies_within_its_static_type((desc, value) in retyped()) {
        for source in sites(&desc, &value) {
            let output = run(&source);
            prop_assert!(
                !output.starts_with("load: ") && !output.contains("error: "),
                "{}\nin\n{}",
                output,
                source
            );
        }
    }

    /// A quote's code or a type value retyped to a code kind or a kind lies within the static type
    /// the load gives it there, at every retype site.
    #[test]
    fn a_retyped_code_or_type_value_lies_within_its_static_type(
        (value, spelled) in code_or_type()
    ) {
        let ascribed = match spelled.starts_with(':') {
            true => spelled.clone(),
            false => format!(":{spelled}"),
        };
        for source in sites_at(&spelled, &ascribed, &value) {
            let output = run(&format!("{SIGNED}{source}"));
            prop_assert!(
                !output.starts_with("load: ") && !output.contains("error: "),
                "{}\nin\n{}",
                output,
                source
            );
        }
    }
}

#[test]
fn every_retype_site_renders_as_source_that_runs() {
    let desc = Desc::Wrapped(Box::new(Desc::Dict(Box::new(Desc::Maybe))));
    let printed = "[:(Wrap {Type = :(MAP Str -> :(Some | None))})({\"a\": Some(1)})]";
    for source in sites(&desc, "(Wrap ({\"a\": (Maybe.Some 1)}))") {
        assert_eq!(run(&source), printed, "{source}");
    }
}

/// A data node read out at a type other than its memo is retyped under a header over the same
/// node, so it stays a knot member and renders its cycle as a cycle, labelled at the top.
#[test]
fn a_retyped_data_node_renders_its_cycle_as_a_cycle() {
    let source = "NEWTYPE Ring = :{next :(LIST OF Any)}\n\
                  LET a = (Ring {next = [a]})\n\
                  LET xs = (ATTR a next)\n\
                  PRINT xs";
    assert_eq!(run(source), "@0 = [Ring({next = @0})]");
}

/// `KIND`, at a name and at an expression.
const KIND: &str = "EXPR #(KIND c :Name) -> Str = #(\"name\")\n\
                    EXPR #(KIND c :Expression) -> Str = #(\"expression\")\n";

/// A quote's code retyped to a code kind dispatches at that kind, at the load and at the run, so a
/// use the kind refuses is refused at load.
#[test]
fn a_quote_retyped_to_a_code_kind_dispatches_at_it() {
    for site in [
        "LET f = (FN :{q :Expression} -> Str = #(KIND q))\nPRINT (f {q = #(y)})",
        "LET q :Expression = #(y)\nPRINT (KIND q)",
        "PRINT (KIND (#(y) :! Expression))",
    ] {
        assert_eq!(run(&format!("{KIND}{site}")), "expression", "{site}");
    }
    assert_eq!(run(&format!("{KIND}PRINT (KIND #(y))")), "name");
    let printed = run("EXPR #(ONLY c :Name) -> Str = #(\"name\")\n\
                       LET f = (FN :{q :Expression} -> Any = #(ONLY q))\n\
                       PRINT (f {q = #(y)})");
    assert!(
        printed.starts_with("load:")
            && printed.contains("no overload of `ONLY _` admits (Expression)"),
        "{printed}"
    );
}

/// A type value retyped to a kind dispatches at that kind: a signature through `t :Type` is no
/// longer a `Signature`.
#[test]
fn a_type_value_retyped_to_a_kind_dispatches_at_it() {
    let source = "EXPR #(SORT t :Signature) -> Str = #(\"signature\")\n\
                  EXPR #(SORT t :Type) -> Str = #(\"type\")\n\
                  SIG Signed = #[(VAL z :Number)]\n\
                  LET g = (FN :{t :Type} -> Str = #(SORT t))\n\
                  PRINT (SORT Signed)\nPRINT (g {t = Signed})\nPRINT (SORT (Signed :! Type))";
    assert_eq!(run(source), "signature\ntype\ntype");
}

/// A quote a cyclic component ties is born at its binder's annotation, so a read through its own
/// edge sees the annotation's kind.
#[test]
fn an_annotated_cyclic_quote_is_born_at_its_annotation() {
    let source = format!(
        "{KIND}LET t :Expression = #($t)\nPRINT (KIND (EVAL t -> Any))\n\
         LET u = #($u)\nPRINT (KIND (EVAL u -> Any))"
    );
    assert_eq!(run(&source), "expression\nname");
}

/// Retyped code runs under `EVAL` as its bare code does, and `USING` over it builds code typed by
/// its own syntax.
#[test]
fn retyped_code_evaluates_and_fills_as_its_syntax() {
    let eval = "LET e = (FN :{q :Expression} -> Any = #(EVAL q -> Any))\nPRINT (e {q = #(1 + 2)})";
    assert_eq!(run(eval), "3");
    let filled = format!(
        "{KIND}LET q :Expression = #(x)\nLET r = (q USING {{x = 1}})\n\
         PRINT (KIND r)\nPRINT (EVAL r -> Number)"
    );
    assert_eq!(run(&filled), "name\n1");
}

/// What running `source` printed, beside how many retype targets the run computed rather than read
/// off the load's records.
fn computed_targets(source: &str) -> (String, usize) {
    use crate::program::CellSubstrate;
    use crate::values::{BuildBudget, TARGETS};
    super::WRITTEN.with(|written| written.borrow_mut().clear());
    let mut substrate = CellSubstrate::load::<super::super::Koan>(
        source,
        "<test>",
        8,
        BuildBudget::default(),
        super::output(),
    )
    .unwrap_or_else(|error| panic!("the program loads: {error}"));
    TARGETS.with(|targets| targets.set(0));
    substrate
        .with(|running| running.run())
        .expect("the program runs");
    let printed = super::WRITTEN.with(|written| written.borrow().join("\n"));
    (printed, TARGETS.with(std::cell::Cell::get))
}

#[test]
fn a_closed_retype_target_is_read_and_never_computed() {
    // Each site retypes a value of a kind to a closed type above its own — an ascription, a `LET`
    // annotation, a parameter, a frame's return, a module's annotated member and its read — and
    // re-stamps by what the load recorded there.
    let sites = [
        ("LET b = ([1 2] :! (LIST OF Any))\nPRINT b", "[1, 2]"),
        ("LET a :(LIST OF Any) = [2]\nPRINT a", "[2]"),
        (
            "LET f = (FN :{x :(LIST OF Any)} -> Any = #(x))\nPRINT (f {x = [1]})",
            "[1]",
        ),
        (
            "LET f = (FN :{x :Any} -> (LIST OF Any) = #([1]))\nPRINT (f {x = 1})",
            "[1]",
        ),
        (
            "MODULE m = ((LET held :(LIST OF Any) = [3]))\nLET d = m.held\nPRINT d",
            "[3]",
        ),
    ];
    for (site, printed) in sites {
        assert_eq!(computed_targets(site), (printed.to_string(), 0), "{site}");
    }
    // A union's members are taken at the value's kind where it runs.
    let union = "LET u = ([1] :! ((LIST OF Any) | Str))\nPRINT u";
    let (printed, computed) = computed_targets(union);
    assert_eq!(printed, "[1]");
    assert!(computed > 0);
}
