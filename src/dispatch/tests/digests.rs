//! Content digests over programs that run: a value is identified by what it holds, a closure or a
//! module by its code as resolved over what it captures, and values that reach one another by the
//! one knot they are tied into.

use crate::program::tests::digest_back;
use std::cell::RefCell;

use crate::program::{CellSubstrate, KBirth, KBundle, KState, Outcome};
use crate::scheduler::{Action, Step, StepError};
use crate::type_lattice::TypeDigest;
use crate::values::HalfHeader;

thread_local! {
    /// What [`interned_digests`] read: every digest the root and the root cell's branch hold.
    static INTERNED: RefCell<Vec<TypeDigest>> = const { RefCell::new(Vec::new()) };
}

/// A root work over the top level's resting view: it records every digest the program's types
/// hold once it has run — the load's, in the root, and the run's, in the root cell's branch, where
/// every top-level evaluation's result crossed.
fn interned_digests<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>,
) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let KState::Born(birth @ KBirth::Inspect { program, .. }) = state else {
        return step.failed(StepError::Refused);
    };
    let mut digests = Vec::new();
    program.root().each_digest(|digest| digests.push(digest));
    if let Some(branch) = step.header().get().map(HalfHeader::branch) {
        branch.each_digest(|digest| digests.push(digest));
    }
    INTERNED.with(|interned| *interned.borrow_mut() = digests);
    step.leave(birth)
}

use super::{Koan, output};
use crate::values::SpecBounds;

/// The content digest of each of `names`, top-level bindings of `source` once it has run.
fn digests<const N: usize>(source: &str, names: [&str; N]) -> [String; N] {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, SpecBounds::default(), output())
            .unwrap_or_else(|error| panic!("the program loads: {error}"));
    assert_eq!(
        substrate.with(|running| running.run()),
        Ok(Outcome::Completed)
    );
    names.map(|name| digest_back(&mut substrate, name))
}

#[test]
fn equal_values_digest_alike_wherever_they_were_built() {
    let [a, b, c, r, s, t] = digests(
        "LET a = [1 \"x\" {k = true}]\nLET b = [1 \"x\" {k = true}]\nLET c = [1 \"x\" {k = false}]\n\
         LET r = {p = 1, q = 2}\nLET s = {q = 2, p = 1}\nLET t = (r :! :{p :Any, q :Any})",
        ["a", "b", "c", "r", "s", "t"],
    );
    assert_eq!(a, b, "two lists of one content, built apart");
    assert_ne!(a, c);
    assert_eq!(r, s, "a record's field order is blind");
    assert_ne!(r, t, "a retype changes the digest");
}

#[test]
fn a_closure_is_its_code_over_what_it_captures() {
    let [a, b, c] = digests(
        "LET make = (FN :{x :Any} -> Any = #(FN :{y :Number} -> Any = #(x)))\n\
         LET a = (make {x = 1})\nLET b = (make {x = 1})\nLET c = (make {x = 2})",
        ["a", "b", "c"],
    );
    assert_eq!(a, b, "two closures of one `FN` over equal captures");
    assert_ne!(a, c, "a capture that differs");
}

/// A read of the top level names the binding it reads, which is bound once per program, so the
/// capture's value is no part of the closure's content.
#[test]
fn a_top_level_capture_is_named_by_its_binding() {
    let read = |value: &str| {
        let [f] = digests(
            &format!("LET t = {value}\nLET f = (FN :{{}} -> Any = #(t))"),
            ["f"],
        );
        f
    };
    assert_eq!(read("5"), read("6"));
}

#[test]
fn functions_that_call_one_another_digest_as_one_knot() {
    let ring = |answer: &str| {
        digests(
            &format!(
                "LET f = (FN :{{}} -> Any = #(g {{}}))\nLET g = (FN :{{}} -> Any = #({answer}))"
            ),
            ["f", "g"],
        )
    };
    let [f, g] = ring("f {}");
    assert_ne!(f, g, "two members of one knot");
    let [other, _] = ring("1");
    assert_ne!(f, other, "a fellow's code is part of the knot");
}

#[test]
fn a_module_is_its_code_over_what_it_captures() {
    let [a, b, c, d] = digests(
        "LET mk = (FN :{x :Any, u :Any} -> Any = #(MODULE m OVER #[x u] = (LET v = x)))\n\
         LET a = (mk {x = 1, u = 1})\nLET b = (mk {x = 1, u = 1})\n\
         LET c = (mk {x = 2, u = 1})\nLET d = (mk {x = 1, u = 2})",
        ["a", "b", "c", "d"],
    );
    assert_eq!(a, b, "two evaluations over equal captures");
    assert_ne!(a, c, "a capture the body reads");
    assert_ne!(
        a, d,
        "an `OVER` entry the body never reads is captured all the same"
    );
}

#[test]
fn a_top_level_over_entry_counts_by_its_binding() {
    let [meters, feet, plain, reads, listed] = digests(
        "LET metric = 1\nLET imperial = 2\n\
         MODULE meters OVER #[metric] = (LET v = 0)\n\
         MODULE feet OVER #[imperial] = (LET v = 0)\n\
         MODULE plain = (LET v = 0)\n\
         MODULE reads = (LET v = metric)\n\
         MODULE listed OVER #[metric] = (LET v = metric)",
        ["meters", "feet", "plain", "reads", "listed"],
    );
    assert_ne!(meters, feet, "one body listing two unread names");
    assert_ne!(meters, plain);
    assert_eq!(
        reads, listed,
        "listing a name the body reads changes nothing"
    );
}

#[test]
fn one_text_whose_use_resolves_apart_digests_apart() {
    let [one, two, again] = digests(
        "EXPR #(HELPER x :Any) -> Any = #(1)\n\
         MODULE one = (LET v = (HELPER 1))\n\
         EXPR #(HELPER x :Number) -> Any = #(2)\n\
         MODULE two = (LET v = (HELPER 1))\n\
         MODULE again = (LET v = (HELPER 1))",
        ["one", "two", "again"],
    );
    assert_ne!(
        one, two,
        "a keyworded use sees only the definitions above it"
    );
    assert_eq!(two, again, "one text resolving alike, at two places");
}

#[test]
fn a_view_is_its_operator_and_application_over_its_source() {
    let [first, second, retyped, other] = digests(
        "SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
         MODULE ints = (LET zero = 0)\nMODULE strs = (LET zero = \"\")\n\
         LET first = (ints :| Counter)\nLET second = (ints :| Counter)\n\
         LET retyped = (ints :! Counter)\nLET other = (strs :| Counter)",
        ["first", "second", "retyped", "other"],
    );
    assert_eq!(first, second, "two evaluations of one ascription");
    assert_ne!(first, retyped, "a view is no header");
    assert_ne!(first, other, "the source");
}

/// Interning is deterministic: one program loaded and run twice, into two registries, interns one
/// set of types. The program builds a carrier and a signature, a scheme, a recursive union's sealed
/// members and a ranked expression shape.
#[test]
fn one_program_loaded_twice_interns_one_digest_set() {
    const PROGRAM: &str = r##"SIG Ordered FOR ALL #[Carrier] = #[(VAL compare :Carrier)]
MODULE ints = (LET compare = 5)
LET view = (ints :| Ordered)
LET kept :(view.Carrier) = view.compare
MODULE lib = (LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x)))
UNION Nat = #{Zero: Null, Succ: Nat}
EXPR #(MOVE 2 TO 1)
EXPR #(MOVE x :Any TO y :Number) -> Str = #("to a number")
PRINT kept
PRINT (MOVE 1 TO 1)"##;
    let interned = || {
        let mut substrate =
            CellSubstrate::load::<Koan>(PROGRAM, "<test>", 8, SpecBounds::default(), output())
                .unwrap_or_else(|error| panic!("the program loads: {error}"));
        substrate.with(|running| {
            assert_eq!(running.run(), Ok(Outcome::Completed));
            running
                .inspect(interned_digests)
                .expect("the inspection runs to completion");
        });
        let mut digests = INTERNED.with(|interned| interned.take());
        digests.sort();
        digests
    };
    assert_eq!(interned(), interned());
}

/// The recipes the load digests a literal and a type value through give what the run's walk gives
/// the value the literal evaluates to.
#[test]
fn a_literal_digests_as_the_value_it_evaluates_to() {
    use crate::parse::KLiteral;
    use crate::type_lattice::KType;
    use crate::values::digest::{literal_digest, type_digest};
    let [number, text, flag, null, named] = digests(
        "LET a = 1.5\nLET b = \"x\"\nLET c = true\nLET d = null\nLET e = Number",
        ["a", "b", "c", "d", "e"],
    );
    assert_eq!(
        number,
        format!("{:?}", literal_digest(&KLiteral::Number(1.5)))
    );
    assert_eq!(
        text,
        format!("{:?}", literal_digest(&KLiteral::String("x")))
    );
    assert_eq!(
        flag,
        format!("{:?}", literal_digest(&KLiteral::Boolean(true)))
    );
    assert_eq!(null, format!("{:?}", literal_digest(&KLiteral::Null)));
    let kind = KType::of_kind(crate::type_lattice::KKind::ProperType);
    assert_eq!(named, format!("{:?}", type_digest(KType::NUMBER, kind)));
}

/// A header digests over its bare member: two modules of one body, branded apart by what they
/// capture, digest apart under one signature that hides the brand, and a header digests apart
/// from its own member.
#[test]
fn headers_over_modules_branded_apart_digest_apart() {
    let [m, f, bare, again] = digests(
        "SIG Plain = #[(VAL zero :Number)]\n\
         LET metric = 1\nLET imperial = 2\n\
         MODULE meters OVER #[metric] = (LET zero = 0)\n\
         MODULE feet OVER #[imperial] = (LET zero = 0)\n\
         LET m = (meters :! Plain)\nLET f = (feet :! Plain)\nLET again = (meters :! Plain)",
        ["m", "f", "meters", "again"],
    );
    assert_ne!(m, f, "a header keeps the brand its signature hides");
    assert_ne!(m, bare, "a retype changes the digest");
    assert_eq!(m, again, "one retype of one member digests alike");
}
