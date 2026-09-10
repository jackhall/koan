//! Inference-walk laws: which slot of a recognized form binds, which uses, which labels, and how a
//! block's positions decide what escapes it.

use std::collections::HashSet;

use proptest::prelude::*;

use super::{CLOSE_RULES, DynamicNameForm, FormRule, infer_close_captures};
use crate::builtins::test_support::TestRun;
use crate::machine::model::render_label;
use crate::memory::{ProgramStorage, program_storage, run_root_storage};
use crate::parse::forms::{FormId, KeyElementSpec, render_key};

/// The [`FORMS`](crate::parse::forms::FORMS) entry a rule tags.
fn form_of(id: FormId) -> &'static crate::parse::forms::Form {
    crate::parse::forms::FORMS
        .iter()
        .find(|form| form.id == id)
        .expect("every tag names a table entry")
}

/// Each rule tags one form, and every slot it claims is a slot position of that form's key. Two
/// entries under one tag would make the walk's reading depend on table order; a claim pointing past
/// the run, or at one of the form's keywords, would read the wrong part.
///
/// The [`FORMS`] half of the same law — tag order, key distinctness, the lazy and type-slot indices
/// — lives with the table, in `parse::forms::tests::table`.
///
/// [`FORMS`]: crate::parse::forms::FORMS
#[test]
fn every_close_rule_tags_one_form_and_claims_its_slot_positions() {
    for (index, (id, rule)) in CLOSE_RULES.iter().enumerate() {
        for (other, _) in &CLOSE_RULES[index + 1..] {
            assert!(
                other != id,
                "two close rules share the form {:?}",
                render_key(form_of(*id).key)
            );
        }

        let form = form_of(*id);
        let claimed: Vec<usize> = match rule {
            FormRule::Signature { signature, body } => {
                std::iter::once(*signature).chain(*body).collect()
            }
            FormRule::Operator { body, .. } => vec![*body],
            FormRule::Arms { arms } | FormRule::MemberArms { arms } => vec![*arms],
            FormRule::ModuleBody { body } => vec![*body],
            FormRule::Attribute { field } => vec![*field],
            FormRule::Projection { fields } => vec![*fields],
            FormRule::ExplicitClose { captures, body } => vec![*captures, *body],
            FormRule::InferredClose { body } => vec![*body],
            FormRule::Dynamic(_) => vec![],
        };
        for index in claimed {
            assert!(
                index < form.key.len(),
                "close rule {:?} claims slot {index} past its run",
                render_key(form.key)
            );
            assert!(
                matches!(form.key[index], KeyElementSpec::Slot),
                "close rule {:?} claims its keyword position {index}",
                render_key(form.key)
            );
        }
    }
}

// ---------- the walk ----------

/// What `block` — the source a `CLOSE` body slot would hold — infers: the free value names, the
/// free type names, and the dynamic-name conflict if it hit one.
struct Inferred {
    values: Vec<String>,
    types: Vec<String>,
    conflict: Option<DynamicNameForm>,
}

fn infer(block: &str) -> Inferred {
    let program: ProgramStorage = program_storage();
    let storage = run_root_storage();
    let run = TestRun::silent(&program, &storage);
    infer_in(&run, block)
}

/// [`infer`] against a run already standing, so a property sweeping many sources seeds the builtin
/// chain once instead of once per source.
fn infer_in(run: &TestRun<'_>, block: &str) -> Inferred {
    let expression = run.parse_one(block);
    let registries = run.registries();
    let inference = infer_close_captures(&expression, run.brand().allocator(), registries);
    Inferred {
        values: inference
            .values
            .iter()
            .map(|name| render_label(name.symbol(), registries))
            .collect(),
        types: inference
            .types
            .iter()
            .map(|name| render_label(name.symbol(), registries))
            .collect(),
        conflict: inference.conflict.map(|conflict| conflict.form),
    }
}

// ---------- positional visibility ----------

/// One statement of a generated block.
#[derive(Clone, Debug)]
enum BlockStatement {
    /// `(LET <name> = <read>)`, or `(LET <name> = 1)` when it reads nothing. The right-hand side is
    /// walked before the binding takes effect.
    Bind { name: String, reads: Option<String> },
    /// `(<name>)` — a bare read.
    Read(String),
    /// `(<left> + <right>)` — a flat expression spelling two names.
    ReadPair(String, String),
}

/// The names a block draws from — a pool small enough that shadowing and repetition happen.
fn pooled_name() -> impl Strategy<Value = String> {
    prop::sample::select(vec!["pa".to_string(), "pb".to_string(), "pc".to_string()])
}

fn block_statement() -> impl Strategy<Value = BlockStatement> {
    prop_oneof![
        2 => (pooled_name(), prop::option::of(pooled_name()))
            .prop_map(|(name, reads)| BlockStatement::Bind { name, reads }),
        1 => pooled_name().prop_map(BlockStatement::Read),
        1 => (pooled_name(), pooled_name())
            .prop_map(|(left, right)| BlockStatement::ReadPair(left, right)),
    ]
}

impl BlockStatement {
    fn source(&self) -> String {
        match self {
            BlockStatement::Bind {
                name,
                reads: Some(read),
            } => format!("(LET {name} = {read})"),
            BlockStatement::Bind { name, reads: None } => format!("(LET {name} = 1)"),
            BlockStatement::Read(name) => format!("({name})"),
            BlockStatement::ReadPair(left, right) => format!("({left} + {right})"),
        }
    }

    /// The names this statement reads, in source order.
    fn reads(&self) -> Vec<&str> {
        match self {
            BlockStatement::Bind {
                reads: Some(read), ..
            } => vec![read],
            BlockStatement::Bind { reads: None, .. } => Vec::new(),
            BlockStatement::Read(name) => vec![name],
            BlockStatement::ReadPair(left, right) => vec![left, right],
        }
    }

    fn binds(&self) -> Option<&str> {
        match self {
            BlockStatement::Bind { name, .. } => Some(name),
            _ => None,
        }
    }
}

fn block_source(statements: &[BlockStatement]) -> String {
    let mut source = String::from("(");
    for statement in statements {
        source.push_str(&statement.source());
    }
    source.push(')');
    source
}

/// The free set the positional rule gives: a name is free iff it is read at a statement no later
/// than the one that binds it, or never bound in the block at all. Reported once, in first-read
/// order.
fn free_names(statements: &[BlockStatement]) -> Vec<String> {
    let mut bound: HashSet<String> = HashSet::new();
    let mut free: Vec<String> = Vec::new();
    for statement in statements {
        for read in statement.reads() {
            if !bound.contains(read) && !free.iter().any(|name| name == read) {
                free.push(read.to_string());
            }
        }
        if let Some(name) = statement.binds() {
            bound.insert(name.to_string());
        }
    }
    free
}

// ---------- form slot roles ----------

/// One recognized form, spelled out, with the role each name it mentions plays.
struct FormTemplate {
    id: FormId,
    /// `{a}`, `{b}` and `{c}` are filled with three distinct fresh names.
    source: &'static str,
    /// Names the form declares: a read of one inside the form is not free outside it.
    bound: &'static [&'static str],
    /// Names in a use slot: free in the enclosing scope.
    free: &'static [&'static str],
    /// Names in a label position: a member, a field or a severed body's own name, free in neither
    /// channel and shadowing nothing.
    labels: &'static [&'static str],
    /// The dynamic-name conflict the form raises, if any.
    conflict: Option<DynamicNameForm>,
}

const TEMPLATES: &[FormTemplate] = &[
    FormTemplate {
        id: FormId::LambdaType,
        source: "FN :{{a} :Number} -> Number",
        bound: &[],
        free: &[],
        labels: &["{a}"],
        conflict: None,
    },
    FormTemplate {
        id: FormId::Lambda,
        source: "FN :{{a} :Number} -> Number = ({a} + {b})",
        bound: &["{a}"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::ExpressionHead,
        source: "EXPR (KAPOW {a} :Number) -> Number",
        bound: &[],
        free: &[],
        labels: &["{a}"],
        conflict: None,
    },
    FormTemplate {
        id: FormId::ExpressionDefinition,
        source: "EXPR (KAPOW {a} :Number) -> Number = ({a} + {b})",
        bound: &["{a}"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::CombinedExpression,
        source: "LET zfun = FN EXPR (KAPOW {a} :Number) -> Number = ({a} + {b})",
        bound: &["{a}"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::QuantifiedExpressionHead,
        source: "EXPR FOR ALL (Elt) (KAPOW {a} :Elt) -> Elt",
        bound: &[],
        free: &[],
        labels: &["{a}"],
        conflict: None,
    },
    FormTemplate {
        id: FormId::QuantifiedExpressionDefinition,
        source: "EXPR FOR ALL (Elt) (KAPOW {a} :Elt) -> Elt = ({a} + {b})",
        bound: &["{a}"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::CombinedQuantifiedExpression,
        source: "LET zfun = FN EXPR FOR ALL (Elt) (KAPOW {a} :Elt) -> Elt = ({a} + {b})",
        bound: &["{a}"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::OperatorDefinition,
        source: "OP #(⊕) OVER Number = (left + right + {b})",
        bound: &["left", "right"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::OperatorDefinitionReturning,
        source: "OP #(⊗) OVER Number -> Number = (left + right + {b})",
        bound: &["left", "right"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::UnaryOperatorDefinition,
        source: "UNARY OP #(≺) OVER Number = (operands + {b})",
        bound: &["operands"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::UnaryOperatorDefinitionReturning,
        source: "UNARY OP #(⊸) OVER Number -> Number = (operands + {b})",
        bound: &["operands"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::CombinedOperator,
        source: "LET zop = OP #(⊛) OVER Number = (left + right + {b})",
        bound: &["left", "right"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::CombinedOperatorReturning,
        source: "LET zop = OP #(⊙) OVER Number -> Number = (left + right + {b})",
        bound: &["left", "right"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::CombinedUnaryOperator,
        source: "LET zop = UNARY OP #(⊚) OVER Number = (operands + {b})",
        bound: &["operands"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::CombinedUnaryOperatorReturning,
        source: "LET zop = UNARY OP #(⊝) OVER Number -> Number = (operands + {b})",
        bound: &["operands"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::Match,
        source: "MATCH {b} -> Number WITH (Some -> (it) None -> (1))",
        bound: &["it"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::Try,
        source: "TRY ({b}) -> Number WITH (Error -> (it))",
        bound: &["it"],
        free: &["{b}"],
        labels: &["Error"],
        conflict: None,
    },
    FormTemplate {
        id: FormId::MatchOver,
        source: "MATCH {b} OVER Shape -> Number WITH (Circle -> (it))",
        bound: &["it"],
        free: &["{b}"],
        labels: &["Circle"],
        conflict: None,
    },
    FormTemplate {
        id: FormId::Module,
        source: "MODULE zmod = ((LET {a} = 1) ({a} + {b}))",
        bound: &["{a}"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::GroupFoldLeft,
        source: "GROUP zgrp FOLD LEFT = ((LET {a} = 1) ({a} + {b}))",
        bound: &["{a}"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::GroupFoldRight,
        source: "GROUP zgrp FOLD RIGHT = ((LET {a} = 1) ({a} + {b}))",
        bound: &["{a}"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::GroupPairwiseFoldLeft,
        source: "GROUP zgrp PAIRWISE FOLD ({c}) LEFT = ((LET {a} = 1) ({a} + {b}))",
        bound: &["{a}"],
        free: &["{b}", "{c}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::GroupPairwiseFoldRight,
        source: "GROUP zgrp PAIRWISE FOLD ({c}) RIGHT = ((LET {a} = 1) ({a} + {b}))",
        bound: &["{a}"],
        free: &["{b}", "{c}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::Attribute,
        source: "{b}.{c}",
        bound: &[],
        free: &["{b}"],
        labels: &["{c}"],
        conflict: None,
    },
    FormTemplate {
        id: FormId::Projection,
        source: "({a} {c}) FROM {b}",
        bound: &[],
        free: &["{b}"],
        labels: &["{a}", "{c}"],
        conflict: None,
    },
    FormTemplate {
        id: FormId::CloseOver,
        source: "CLOSE OVER ({b}) ({a})",
        bound: &[],
        free: &["{b}"],
        labels: &["{a}"],
        conflict: None,
    },
    FormTemplate {
        id: FormId::Close,
        source: "CLOSE ((LET {a} = 1) ({a} + {b}))",
        bound: &["{a}"],
        free: &["{b}"],
        labels: &[],
        conflict: None,
    },
    FormTemplate {
        id: FormId::UsingScope,
        source: "USING zmod SCOPE ({b})",
        bound: &[],
        free: &[],
        labels: &[],
        conflict: Some(DynamicNameForm::Using),
    },
    FormTemplate {
        id: FormId::Eval,
        source: "EVAL {b}",
        bound: &[],
        free: &[],
        labels: &[],
        conflict: Some(DynamicNameForm::Eval),
    },
];

/// The names a template's placeholders draw from — disjoint from every name the templates spell
/// themselves, so a role assertion is never satisfied by accident.
const FILLERS: &[&str] = &["qa", "qb", "qc", "qd", "va", "vb", "vc"];

/// `text` with the three placeholders filled.
fn fill(text: &str, names: &[&str]) -> String {
    text.replace("{a}", names[0])
        .replace("{b}", names[1])
        .replace("{c}", names[2])
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// A name is free iff it is read at a statement no later than the one binding it, or never
    /// bound in the block at all — the positional rule, with the cutoff strict so a binder does not
    /// bind its own right-hand side. Each free name is reported once however often it is spelled.
    ///
    /// A module body is the exception the rule is stated against: its type declarations are
    /// announced body-wide, so a mutually recursive cycle infers neither name in any source order.
    #[test]
    fn a_name_is_free_iff_no_earlier_statement_binds_it(
        statements in prop::collection::vec(block_statement(), 2..6),
        order in (2usize..4).prop_flat_map(|count| {
            Just((0..count).collect::<Vec<usize>>()).prop_shuffle()
        }),
    ) {
        let inferred = infer(&block_source(&statements));
        prop_assert_eq!(inferred.values, free_names(&statements));
        prop_assert!(inferred.types.is_empty());
        prop_assert!(inferred.conflict.is_none());

        let count = order.len();
        let declarations: String = order
            .iter()
            .map(|index| {
                let next = (index + 1) % count;
                format!("(NEWTYPE Za{index} = :{{fld :Za{next}}})")
            })
            .collect();
        let announced = infer(&format!("((MODULE zmod = ({declarations})) (zmod))"));
        prop_assert!(announced.types.is_empty(), "{:?}", announced.types);
        prop_assert!(announced.values.is_empty(), "{:?}", announced.values);
    }

    /// Every recognized form reads its slots by role: a name the form declares is never free, a
    /// name in a use slot always is, and a name in a label position — a member head, a field, a
    /// severed body's own name — is free in neither channel. A form the domain forbids raises its
    /// conflict instead, and no other form raises one.
    ///
    /// The template table covers `CLOSE_RULES` exactly, so a rule added without a spelled surface
    /// fails here rather than going unread.
    #[test]
    fn every_form_reads_its_slots_by_role(
        names in prop::sample::subsequence(FILLERS.to_vec(), 3),
    ) {
        let tagged: HashSet<usize> = TEMPLATES.iter().map(|entry| entry.id as usize).collect();
        prop_assert_eq!(tagged.len(), TEMPLATES.len(), "a form is templated twice");
        prop_assert_eq!(
            tagged,
            CLOSE_RULES.iter().map(|(id, _)| *id as usize).collect::<HashSet<_>>(),
        );

        let program = program_storage();
        let storage = run_root_storage();
        let run = TestRun::silent(&program, &storage);
        for template in TEMPLATES {
            let source = format!("(({}) (1))", fill(template.source, &names));
            let inferred = infer_in(&run, &source);

            prop_assert_eq!(inferred.conflict, template.conflict, "{}", source);
            for name in template.bound {
                let filled = fill(name, &names);
                prop_assert!(!inferred.values.contains(&filled), "{source}: {filled} is free");
            }
            for name in template.free {
                let filled = fill(name, &names);
                prop_assert!(inferred.values.contains(&filled), "{source}: {filled} is not free");
            }
            for name in template.labels {
                let filled = fill(name, &names);
                prop_assert!(!inferred.values.contains(&filled), "{source}: {filled} is a free value");
                prop_assert!(!inferred.types.contains(&filled), "{source}: {filled} is a free type");
            }
        }
    }

    /// The frontiers: an explicit `CLOSE OVER` severs its body, so what the body spells is its own
    /// business and no conflict inside it reaches out; a nested inferred `CLOSE` contributes the
    /// free set of its own block and keeps its own conflict; a quote in an eager position is data.
    /// A conflict anywhere the enclosing block *would* evaluate is found, and the sigil and spelled
    /// spellings of one form report the same one.
    #[test]
    fn severing_slots_and_nested_closes_carry_their_own_frontiers(
        statements in prop::collection::vec(block_statement(), 2..6),
        capture in pooled_name(),
        dynamic in "[a-z]{2,3}",
    ) {
        let block = block_source(&statements);
        let free = free_names(&statements);

        let explicit = infer(&format!("(CLOSE OVER ({capture}) {block})"));
        prop_assert_eq!(explicit.values, vec![capture.clone()]);

        let nested = infer(&format!("(CLOSE {block})"));
        prop_assert_eq!(nested.values, free);

        let quoted = infer(&format!("(#{block})"));
        prop_assert!(quoted.values.is_empty());

        for spelling in [format!("$({dynamic})"), format!("EVAL {dynamic}")] {
            prop_assert_eq!(
                infer(&format!("(FN :{{}} -> Any = ({spelling}))")).conflict,
                Some(DynamicNameForm::Eval),
                "{}",
                spelling,
            );
            prop_assert!(
                infer(&format!("(CLOSE OVER ({capture}) ({spelling}))")).conflict.is_none(),
                "{}",
                spelling,
            );
            prop_assert!(
                infer(&format!("(CLOSE ({spelling}))")).conflict.is_none(),
                "{}",
                spelling,
            );
        }
    }
}

/// A nominal declaration's own name is visible inside its representation, so a self-recursive type
/// infers nothing — even though the positional rule alone would call the name free.
#[test]
fn a_self_recursive_nominal_declares_its_own_name() {
    let inferred = infer("((NEWTYPE Tree = :{left :Tree}) (1))");
    assert!(inferred.types.is_empty(), "{:?}", inferred.types);
    assert!(inferred.values.is_empty());
}

/// The same rule through a module body's pre-announcement: an announced `UNION`'s owned tags are
/// not body-wide names, so a sibling declaration using one names an outer type.
#[test]
fn an_announced_unions_tags_are_not_body_wide_names() {
    let inferred = infer(
        "((MODULE m = ((UNION Shape = (Circle :Number)) (NEWTYPE Ring = :{c :Circle}))) (m))",
    );
    assert_eq!(inferred.types, ["Number", "Circle"]);
}

/// `$(…)` resolves its names at evaluation, so the block has no inferable capture list.
#[test]
fn an_eval_form_is_a_conflict() {
    assert_eq!(infer("($(e))").conflict, Some(DynamicNameForm::Eval));
}

/// `USING … SCOPE` surfaces module members dynamically.
#[test]
fn a_using_window_is_a_conflict() {
    assert_eq!(
        infer("(USING m SCOPE (x))").conflict,
        Some(DynamicNameForm::Using)
    );
}
