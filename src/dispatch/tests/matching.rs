//! `MATCH`: what the load types each arm's `it` and the `MATCH` itself as, which arms it keeps and
//! selects, what it refuses, and what a run selects and yields.

use crate::scope::Arms;
use crate::type_lattice::{Interval, KType, Verdict, display_name};
use crate::values::tests::reading;

use super::run;
use super::statics::{body, interval, loaded, top};
use crate::memory::Bump;
use crate::program::Program;

const MAYBE: &str = "UNION Maybe = #{Some: Number, None: Null}";

/// The static type of the binder `name` in the body of the top-level `FN` binder `f` of `source`,
/// rendered as its two ends.
fn in_f(source: &str, name: &str) -> String {
    loaded(source, |program| {
        let typed = interval(program, body(program, program.shape(), "f"), name);
        ends(program, typed)
    })
}

/// `typed`'s ends, rendered `[lower, upper]`, or the one type where they meet.
fn ends(program: &Program<'_>, typed: Interval) -> String {
    let bump = Bump::new();
    let types = reading(program.tiers(), &bump);
    let show = |end| display_name(end, &types, program.symbols()).to_string();
    match typed.lower == typed.upper {
        true => show(typed.upper),
        false => format!("[{}, {}]", show(typed.lower), show(typed.upper)),
    }
}

/// The load's record of the arms of the `MATCH` bound to the top-level `name`.
fn arms_of(source: &str, name: &str, check: impl FnOnce(Arms<'_>)) {
    loaded(source, |program| {
        let shape = program.shape();
        let slot = super::statics::slot(program, shape, name);
        let Some(crate::parse::ExpressionPart::Expression(node)) = shape.rhs(slot) else {
            panic!("`{name}` is bound to a node");
        };
        let arms = (program.facts())
            .arms(shape, shape.site_of_node(node))
            .expect("the load recorded the arms");
        check(arms);
    });
}

#[test]
fn an_arms_it_is_retyped_to_its_guard() {
    let f = |guards: &str| {
        format!("LET f = (FN :{{x :Any}} -> Any = #((LET m = (MATCH x WITH #{{{guards}}})) (m)))")
    };
    assert_eq!(
        in_f(&f("Number: (it), _: (null)"), "m"),
        "[Never, :(Number | Null)]"
    );
    assert_eq!(
        in_f(&f(":(LIST OF Any): (it), _: (null)"), "m"),
        "[Never, :((LIST OF Any) | Null)]",
        "a list guard retypes `it` exactly"
    );
    assert_eq!(
        in_f(&f("\"a\": (it), _: (null)"), "m"),
        "[Never, :(Str | Null)]"
    );
    assert_eq!(top("LET m = (MATCH 1 WITH #{_: (it)})", "m"), "Number");
    assert_eq!(
        top(
            &format!(
                "{MAYBE}\nLET m = (MATCH (Maybe.Some 1) UNDER Maybe WITH #{{Some: (it), None: (0)}})"
            ),
            "m"
        ),
        "Number",
        "a label's `it` is its variant's payload"
    );
    assert_eq!(
        top(
            &format!(
                "{MAYBE}\nLET m = (MATCH (Maybe.None null) UNDER Maybe WITH #{{Some: (1), _: (it)}})"
            ),
            "m"
        ),
        "Null",
        "`_` binds the payload of a variant no label names"
    );
}

#[test]
fn a_match_spans_the_arms_the_load_keeps() {
    let f = "LET f = (FN :{x :(Number | Str)} -> Any = #((LET m = (MATCH x WITH \
             #{Number: (it), Str: (it), Bool: (it)})) (m)))";
    assert_eq!(
        in_f(f, "m"),
        "[Never, :(Number | Str)]",
        "a `Bool` arm never runs"
    );
    assert_eq!(
        top("LET m = (MATCH 1 WITH #{Number: (\"n\"), Any: (1)})", "m"),
        "Str",
        "the arm the load selects is the `MATCH`'s type"
    );
    let same = "LET f = (FN :{x :(Number | Str)} -> Any = #((LET m = (MATCH x WITH \
                #{Number: (1), Str: (2)})) (m)))";
    assert_eq!(
        in_f(same, "m"),
        "Number",
        "exact where every kept arm is exact at one type"
    );
}

#[test]
fn the_load_keeps_the_arms_a_run_may_take() {
    arms_of(
        "LET x = [1]\nLET m = (MATCH (x 0) WITH #{Number: (1), Str: (2), Any: (3)})",
        "m",
        |arms| {
            let kept: Vec<_> = arms
                .kept
                .iter()
                .map(|(at, verdict)| (*at, *verdict))
                .collect();
            assert_eq!(
                kept,
                [(2, Verdict::Always)],
                "only `Any` admits a read's `Result`"
            );
            assert_eq!(arms.chosen, Some(2));
        },
    );
    arms_of(
        "LET f = (FN :{x :Any} -> Any = #(x))\nLET m = (MATCH (f {x = 1}) WITH #{Number: (1), 0: (2), Any: (3)})",
        "m",
        |arms| {
            let kept: Vec<_> = arms
                .kept
                .iter()
                .map(|(at, verdict)| (*at, *verdict))
                .collect();
            assert_eq!(
                kept,
                [
                    (0, Verdict::Maybe),
                    (1, Verdict::Maybe),
                    (2, Verdict::Always)
                ]
            );
            assert_eq!(arms.chosen, None);
        },
    );
}

#[test]
fn a_match_no_arm_of_which_can_run_is_refused() {
    assert_eq!(
        run("MATCH 1 WITH #{Str: (1), Bool: (2)}"),
        "load: <test>:1:1: no arm of `MATCH` admits Number"
    );
    assert_eq!(
        run("MATCH 1 WITH #{:(Number | Str): (1), :(Number | Bool): (2)}"),
        "load: <test>:1:1: ambiguous `MATCH`: 2 arms admit Number and none ranks first"
    );
}

#[test]
fn a_match_under_a_union_is_checked_against_it_where_the_load_closes_it() {
    assert_eq!(
        run(&format!(
            "{MAYBE}\nMATCH (Maybe.Some 1) UNDER Maybe WITH #{{Some: (1), Bogus: (2), None: (3)}}"
        )),
        "load: <test>:2:28: Bogus names no variant of :(Some | None)"
    );
    assert_eq!(
        run(&format!(
            "{MAYBE}\nMATCH (Maybe.Some 1) UNDER Maybe WITH #{{Some: (1)}}"
        )),
        "load: <test>:2:1: this `MATCH … UNDER :(Some | None)` has no arm for None; add a `_` arm \
         to default it"
    );
    assert_eq!(
        run("NEWTYPE Meters = Number\nMATCH (Meters 1) UNDER Meters WITH #{_: (1)}"),
        "load: <test>:2:24: `MATCH … UNDER` takes a union, and Meters is not one; a lone variant is not its \
         union"
    );
    assert_eq!(
        run("MATCH 1 UNDER Number WITH #{_: (1)}"),
        "load: <test>:1:15: `MATCH … UNDER` takes a union, and Number is not one",
        "a structural type is no variant"
    );
    loaded(
        &format!("{MAYBE}\nMATCH (Maybe.None null) UNDER Maybe WITH #{{Some: (1), _: (\"d\")}}"),
        |_| (),
    );
}

#[test]
fn a_match_whose_scrutinee_never_arrives_is_never() {
    let f = "LET f = (FN :{x :Never} -> Any = #((LET m = (MATCH x WITH #{Str: (1)})) (m)))";
    loaded(f, |program| {
        let typed = interval(program, body(program, program.shape(), "f"), "m");
        assert_eq!(typed, Interval::point(KType::NEVER.into()));
    });
}

#[test]
fn a_match_runs_the_most_specific_admitting_arm() {
    let arms = "#{Any: (\"any\"), Number: (\"number\"), :(Number | Str): (\"either\"), _: (\"d\")}";
    assert_eq!(run(&format!("PRINT (MATCH 1 WITH {arms})")), "number");
    assert_eq!(run(&format!("PRINT (MATCH \"s\" WITH {arms})")), "either");
    assert_eq!(
        run(&format!("PRINT (MATCH true WITH {arms})")),
        "any",
        "`_` loses to `Any`"
    );
    assert_eq!(
        run("PRINT (MATCH true WITH #{Number: (1), _: (\"d\")})"),
        "d"
    );
    let f = |arms: &str| {
        format!(
            "LET f = (FN :{{x :Any}} -> Any = #(MATCH x WITH {arms}))\nPRINT (f {{x = 0}})\nPRINT (f {{x = 5}})"
        )
    };
    assert_eq!(
        run(&f("#{0: (\"zero\"), Number: (\"other\")}")),
        "zero\nother"
    );
    assert_eq!(
        run(
            "LET f = (FN :{x :Any} -> Any = #(MATCH x WITH #{true: (\"yes\"), false: (\"no\")}))\n\
             PRINT (f {x = true})\nPRINT (f {x = false})"
        ),
        "yes\nno"
    );
    assert_eq!(
        run(
            "NEWTYPE Meters = Number\nPRINT (MATCH (Meters 1) WITH #{1: (\"one\"), _: (\"tagged\")})"
        ),
        "tagged",
        "a tagged value equals no scalar literal"
    );
}

#[test]
fn a_match_no_arm_of_which_admits_at_run_faults() {
    assert_eq!(
        run("LET f = (FN :{x :Any} -> Any = #(MATCH x WITH #{Str: (1)}))\nPRINT (f {x = 1})"),
        "error: no arm of `MATCH` admits Number"
    );
    assert_eq!(
        run(
            "LET f = (FN :{x :Any} -> Any = #(MATCH x WITH #{:(Number | Str): (1), :(Number | Bool): (2)}))\n\
             PRINT (f {x = 1})"
        ),
        "error: ambiguous `MATCH`: 2 arms admit Number and none ranks first"
    );
}

#[test]
fn an_arms_it_dispatches_at_its_guard() {
    assert_eq!(
        run(
            "EXPR #(KIND xs :(LIST OF Number)) -> Str = #(\"numbers\")\n\
             EXPR #(KIND xs :(LIST OF Any)) -> Str = #(\"anything\")\n\
             PRINT (MATCH [1] WITH #{:(LIST OF Any): (KIND it)})"
        ),
        "anything"
    );
}

#[test]
fn a_match_under_a_union_unwraps_the_variant() {
    let option = "UNION (Elem AS Option) = #{Some: Elem, None: Null}";
    assert_eq!(
        run(&format!(
            "{option}\nPRINT (MATCH (Option.Some 1) UNDER Option WITH #{{Some: (it), None: (0)}})"
        )),
        "1"
    );
    assert_eq!(
        run(&format!(
            "{option}\nLET f = (FN :{{x :(Number AS Option)}} -> Number = #(MATCH x UNDER (Number AS Option) WITH #{{Some: (it), None: (0)}}))\n\
             PRINT (f {{x = (Option.Some 2)}})\nPRINT (f {{x = (Option.None null)}})"
        )),
        "2\n0"
    );
}

#[test]
fn a_rigid_guard_ranks_at_run() {
    let f = "EXPR FOR ALL #[Elt] #(WHICH x :Any y :Elt) -> Str = #(MATCH x WITH #{Elt: (\"elt\"), Number: (\"number\"), _: (\"other\")})";
    assert_eq!(run(&format!("{f}\nPRINT (WHICH 1 \"s\")")), "number");
    assert_eq!(run(&format!("{f}\nPRINT (WHICH \"t\" \"s\")")), "elt");
    let repeated = run(&format!("{f}\nPRINT (WHICH 1 2)"));
    assert!(
        repeated.contains("`Number` guards two arms"),
        "a binding that makes two guards one is refused where it is walked: {repeated}"
    );
}

#[test]
fn a_union_clause_over_a_run_bound_name_is_checked_where_it_is_bound() {
    let f =
        "EXPR FOR ALL #[Un] #(PICK x :Un) -> Any = #(MATCH x UNDER Un WITH #{Some: (it), _: (0)})";
    assert_eq!(
        run(&format!(
            "{MAYBE}\n{f}\nLET m :Maybe = (Maybe.Some 3)\nPRINT (PICK m)"
        )),
        "3"
    );
    let other = "UNION Other = #{Left: Number, Right: Null}";
    let picked = run(&format!(
        "{other}\n{f}\nLET o :Other = (Other.Left 3)\nPRINT (PICK o)"
    ));
    assert!(
        picked.contains("Some names no variant of"),
        "a binding making a label miss refuses: {picked}"
    );
    let direct = run(&format!("{MAYBE}\n{f}\nPRINT (PICK (Maybe.Some 3))"));
    assert!(
        direct.contains("and Some is not one; a lone variant is not its union"),
        "a constructed variant binds `Un` to the variant, no union: {direct}"
    );
}

#[test]
fn a_settlement_types_a_match_over_a_for_all_name_exactly() {
    // At the load `Elt` is rigid, so the ascription may hold; the settlement at `Elt := Number`
    // types the `MATCH` exactly `Number`, which never satisfies `Str`.
    let f = "EXPR FOR ALL #[Elt] #(ONLY x :Elt) -> Any = #((MATCH x WITH #{Elt: (it)}) :! Str)";
    assert_eq!(run(&format!("{f}\nPRINT (ONLY \"s\")")), "s");
    let refused = run(&format!("{f}\nPRINT (ONLY 1)"));
    assert!(
        refused.contains("can never satisfy its ascription Str"),
        "the settlement refuses: {refused}"
    );
}

/// Past the 64 arms a table ranks, arms rank by brute force: at run, over value guards, and at
/// load, over always-admitting type guards.
#[test]
fn a_match_of_more_arms_than_a_table_holds_ranks_them() {
    let values: Vec<String> = (0..70).map(|at| format!("{at}: (\"v{at}\")")).collect();
    let pick = format!(
        "EXPR #(PICKS x :Any) -> Str = #(MATCH x WITH #{{{}, Number: (\"number\")}})\n",
        values.join(", ")
    );
    assert_eq!(run(&format!("{pick}PRINT (PICKS 69)")), "v69");
    assert_eq!(run(&format!("{pick}PRINT (PICKS 100)")), "number");
    let names: Vec<String> = (0..70u8)
        .map(|at| {
            format!(
                "Nt{}{}",
                char::from(b'a' + at / 26),
                char::from(b'a' + at % 26)
            )
        })
        .collect();
    let newtypes: String = (names.iter())
        .map(|name| format!("NEWTYPE {name} = Null\n"))
        .collect();
    let wide =
        |guards: &[String]| format!("{newtypes}PRINT (MATCH 1 WITH #{{{}}})", guards.join(", "));
    let mut guards: Vec<String> = (names.iter())
        .map(|name| format!(":(Number | {name}): (\"{name}\")"))
        .collect();
    let ambiguous = run(&wide(&guards));
    assert!(
        ambiguous.contains("70 arms admit"),
        "none ranks first, refused at load: {ambiguous}"
    );
    guards.push("Number: (\"number\")".to_string());
    assert_eq!(run(&wide(&guards)), "number");
}
