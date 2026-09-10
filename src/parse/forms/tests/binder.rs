//! What a parsed statement caches off the form table: the entry it matched, the binder plan its
//! extractors filled, the declared-name position and the lazy stamp.

use proptest::prelude::*;

use crate::memory::{ProgramBrand, program_storage};
use crate::parse::forms::binder::BinderFacts;
use crate::parse::forms::{FORMS, Form, KeyElementSpec, form_for, render_key};
use crate::parse::labels::Symbol;
use crate::parse::{ExpressionPart, KExpression, LabelInterner, parse};
use crate::source::Spanned;

/// Every form the table gives binder facts, with those facts beside it.
fn binder_forms() -> impl Iterator<Item = (&'static Form, BinderFacts)> {
    FORMS
        .iter()
        .filter_map(|form| form.binder.map(|binder| (form, binder)))
}

/// Every form that installs anything declares at least one channel, and a `names` entry carries the
/// bind kind the placeholder is tagged with. Pins that the binder facts' two channels are the only
/// routes into an install.
///
/// The silent entries are the SIG **declaration** forms — `VAL` and the three bodyless operator
/// heads, each recording into the decl scope's own collectors rather than into a binding map — plus
/// the lambda, which has neither a name nor a head to key a bucket on and is listed only for its
/// type slot. Anything else appearing here means a binder builtin lost its extractor.
#[test]
fn binder_channels_cover_every_installing_form() {
    let silent: Vec<Vec<String>> = binder_forms()
        .filter(|(_, binder)| binder.installs_nothing())
        .map(|(form, _)| render_key(form.key))
        .collect();
    assert_eq!(
        silent,
        vec![
            vec!["VAL", "_", "_"],
            vec!["FN", "_", "->", "_", "=", "_"],
            vec!["OP", "_", "OVER", "_"],
            vec!["OP", "_", "OVER", "_", "->", "_"],
            vec!["UNARY", "OP", "_", "OVER", "_", "->", "_"],
        ],
    );
}

// ---------- the parse-time plan ----------

/// What a declaration's own spine installs, as the surface promises it.
#[derive(Clone, Copy, Debug)]
enum Expectation {
    /// The spine declares nothing at all, so it caches no plan.
    NoPlan,
    /// The spine registers bucket keys and declares no name of its own.
    BucketsOnly,
    /// The spine declares the statement's fresh name plus exactly this many bucket keys.
    Named(usize),
}

/// Declaration surfaces spelled the way a program spells them, so the bucket channel a bare form
/// key cannot exercise is exercised here. `{n}` is the declared name, `{p}` a parameter name and
/// `{o}` an operator glyph, each filled fresh per case.
const DECLARATIONS: &[(&str, Expectation)] = &[
    ("LET {n} = 1", Expectation::Named(0)),
    // A statement's plan is its own spine: what a slot's child would install is not part of it, so
    // the namespace a block introduces is legible from its statement keys alone.
    ("LET {n} = (LET {p} = 3)", Expectation::Named(0)),
    (
        "LET {n} = (EXPR (KAPOW {p} :Number) -> Number = ({p}))",
        Expectation::Named(0),
    ),
    // Each combined form fills both channels: the LET value name and the bucket key(s) the
    // declaration's body registers. `LET … = UNARY OP …` is the two-bucket maximum.
    (
        "LET {n} = FN EXPR (KAPOW {p} :Number) -> Number = ({p})",
        Expectation::Named(1),
    ),
    (
        "LET {n} = OP #({o}) OVER Number = (left + right)",
        Expectation::Named(1),
    ),
    (
        "LET {n} = OP #({o}) OVER Number -> Bool = (left < right)",
        Expectation::Named(1),
    ),
    (
        "LET {n} = UNARY OP #({o}) OVER Number -> :(LIST OF Number) = (operands)",
        Expectation::Named(2),
    ),
    (
        "EXPR (KAPOW {p} :Number) -> Number = ({p})",
        Expectation::BucketsOnly,
    ),
    (
        "OP #({o}) OVER Number = (left + right)",
        Expectation::BucketsOnly,
    ),
    (
        "UNARY OP #({o}) OVER Number -> Number = (0 - operands)",
        Expectation::BucketsOnly,
    ),
    // A `VAL` declaration records into the decl scope's collector, and the anonymous `FN :{…}`
    // signature names no bucket, so neither caches a plan.
    ("VAL {n} :Number", Expectation::NoPlan),
    ("FN :{{p} :Number} -> Number = ({p})", Expectation::NoPlan),
];

/// The operator glyphs a generated declaration draws from — each keyword-class and unclaimed by the
/// seeded root, so a case registers rather than shadowing.
const GLYPHS: &[&str] = &["⊕", "⊗", "≺", "⊸", "⊛"];

/// The lone top-level statement `source` parses to, with its cache filled.
fn parse_one<'a>(brand: ProgramBrand<'a>, source: &str) -> KExpression<'a> {
    parse(brand, &LabelInterner::new(), source)
        .expect("the rendered form parses")
        .into_iter()
        .next()
        .expect("one statement")
}

/// A form key spelled out: keywords verbatim, each slot filled with `filler`.
fn render_form(form: &Form, fillers: &[String]) -> String {
    let mut slot = 0;
    let mut out = Vec::new();
    for element in form.key {
        match element {
            KeyElementSpec::Keyword(name) => out.push(name.text().to_string()),
            KeyElementSpec::Slot => {
                out.push(fillers[slot % fillers.len()].clone());
                slot += 1;
            }
        }
    }
    out.join(" ")
}

/// The bare name token at `index`, if the position holds one.
fn name_token_at(statement: &KExpression<'_>, index: usize) -> Option<Symbol> {
    match statement.parts.get(index)?.value {
        ExpressionPart::Identifier(name) => Some(name.symbol()),
        ExpressionPart::Type(name) => Some(name.symbol()),
        _ => None,
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// A parsed builtin form caches the table entry its key matches, and every fact the node
    /// answers off that entry is the entry's: the declared-name position is the form's
    /// `name_slot`, the lazy stamp is the form's, and where the extractors did find a name the
    /// token at that position **is** that name. A redundant single-`Expression` paren wrapper is
    /// not itself a binder — the submission path reads through it to the child.
    ///
    /// A key the table does not spell caches nothing at all: no entry, no plan, no name position,
    /// and no lazy stamp, so raw capture stays available to builtin registration alone.
    #[test]
    fn a_parsed_form_caches_its_entry_and_a_user_key_caches_nothing(
        value_fillers in prop::collection::vec("[a-z]{2,4}", 1..4),
        type_fillers in prop::collection::vec("[A-Z][a-z]{1,3}", 1..4),
        stranger in prop::collection::vec("[a-z]{2,4}", 1..4),
        stranger_keyword in "[A-Z]{5,7}",
    ) {
        let program = program_storage();
        let brand = program.brand();

        for (form, fillers) in FORMS
            .iter()
            .flat_map(|form| [(form, &value_fillers), (form, &type_fillers)])
        {
            let source = render_form(form, fillers);
            let statement = parse_one(brand, &source);
            let cached = statement
                .cache()
                .form()
                .unwrap_or_else(|| panic!("{source}: a spelled form key matches its entry"));
            prop_assert!(
                std::ptr::eq(cached, form),
                "{} parses to the entry {:?}",
                source,
                render_key(cached.key),
            );

            prop_assert_eq!(
                statement.binder_name_slot(),
                form.binder.and_then(|binder| binder.name_slot),
            );
            for slot in 0..statement.parts.len() {
                prop_assert_eq!(statement.lazy_kinds_at(slot), form.lazy_kinds_at(slot));
            }
            if let Some(name) = statement.binder_plan().and_then(|plan| plan.name) {
                let at = statement
                    .binder_name_slot()
                    .expect("a plan carrying a name comes from a form with a name slot");
                prop_assert_eq!(name_token_at(&statement, at), Some(name.symbol()), "{}", source);
            }

            // The redundant wrapper carries the child's plan through, with no aggregation.
            let wrapped = KExpression::new(
                brand.region(),
                &[Spanned::bare(ExpressionPart::Expression(
                    brand.nested_node(statement.parts),
                ))],
            );
            prop_assert!(wrapped.binder_plan().is_none());
            let ExpressionPart::Expression(child) = wrapped.parts[0].value else {
                panic!("built a single-Expression wrapper");
            };
            prop_assert_eq!(
                child.binder_plan().and_then(|plan| plan.name),
                statement.binder_plan().and_then(|plan| plan.name),
            );
        }

        // A run the table does not spell: a long fresh keyword followed by fresh identifiers.
        let mut run = vec![crate::builtins::test_support::kw_part(&stranger_keyword)];
        run.extend(
            stranger
                .iter()
                .map(|name| crate::builtins::test_support::identifier_part(name)),
        );
        let user = KExpression::new_from_iter(
            brand.region(),
            run.into_iter().map(Spanned::bare),
        );
        prop_assume!(form_for(user.stored_key().iter().copied()).is_none());
        prop_assert!(user.cache().form().is_none());
        prop_assert!(user.binder_plan().is_none());
        prop_assert!(user.binder_name_slot().is_none());
        for slot in 0..user.parts.len() {
            prop_assert!(user.lazy_kinds_at(slot).is_empty());
        }
    }

    /// A declaration's parse-time plan is its own spine and nothing else: the name channel carries
    /// what the statement itself declares, the bucket channel the keys its declaration body
    /// registers, and a form that installs through neither caches no plan at all.
    #[test]
    fn a_declarations_plan_is_its_own_spine(
        name in "[a-z]{2,4}",
        parameter in "[a-z]{2,4}",
        glyph in 0..GLYPHS.len(),
    ) {
        let program = program_storage();
        let brand = program.brand();

        for &(template, expectation) in DECLARATIONS {
        let source = template
            .replace("{n}", &name)
            .replace("{p}", &parameter)
            .replace("{o}", GLYPHS[glyph]);

        let statement = parse_one(brand, &source);
        let plan = statement.binder_plan();

        match expectation {
            Expectation::NoPlan => prop_assert!(plan.is_none(), "{}", source),
            Expectation::BucketsOnly => {
                let plan = plan.unwrap_or_else(|| panic!("{source}: a declaration is a binder"));
                prop_assert!(plan.name.is_none(), "{}", source);
                prop_assert!(plan.buckets.is_some_and(|keys| keys.count() > 0), "{}", source);
                prop_assert_eq!(statement.binder_name_slot(), None, "{}", source);
            }
            Expectation::Named(buckets) => {
                let plan = plan.unwrap_or_else(|| panic!("{source}: a declaration is a binder"));
                prop_assert_eq!(
                    plan.name.map(|declared| declared.symbol()),
                    Some(Symbol::of(&name)),
                    "{}",
                    source,
                );
                prop_assert_eq!(plan.buckets.map_or(0, |keys| keys.count()), buckets, "{}", source);
            }
        }
        }
    }
}
