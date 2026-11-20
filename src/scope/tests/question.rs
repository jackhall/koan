//! The questions a door asks: the closed door's answer per kind, a binding written into a question
//! whole and in part, whether a binding keeps an answer, and an answered question's digest.

use crate::memory::{Bump, resident};
use crate::symbols::{SymbolInterner, TypeSymbol};
use crate::type_lattice::{DeclaredType, Interval, KKind, KType, Parametric, Verdict};
use crate::values::tests::TestTypes;

use super::super::{Answer, Answered, Levels, Question, answer, keeps};

fn ty(kt: KType) -> DeclaredType<Parametric> {
    kt.into()
}

#[test]
fn the_closed_door_answers_each_kind_holding_no_type() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let asked = |question: Question<'_>| answer(&types, &scratch, &question);
    let (number, str, any) = (KType::NUMBER, KType::STR, KType::ANY);
    assert_eq!(
        asked(Question::Fits {
            a: ty(number),
            b: ty(any)
        }),
        Some(Answer::Bool(true))
    );
    assert_eq!(
        asked(Question::Fits {
            a: ty(any),
            b: ty(number)
        }),
        Some(Answer::Bool(false))
    );
    assert_eq!(
        asked(Question::Same {
            a: ty(number),
            b: ty(str)
        }),
        Some(Answer::Bool(false))
    );
    assert_eq!(
        asked(Question::LowerEndOutside {
            value: Interval::point(number.into()),
            bound: ty(str),
        }),
        Some(Answer::Bool(true))
    );
    assert_eq!(
        asked(Question::Kind { ty: number.into() }),
        Some(Answer::Kind(KKind::ProperType))
    );
    // A refusal is worded from the types its question stored, so *never* is an answer like any
    // other.
    assert_eq!(
        asked(Question::NeverSatisfies {
            value: Interval::point(number.into()),
            wanted: str.into(),
        }),
        Some(Answer::Never)
    );
    assert_eq!(
        asked(Question::NeverSatisfies {
            value: Interval::point(number.into()),
            wanted: any.into(),
        }),
        Some(Answer::Satisfiable)
    );
    // A verdict is always written: its intervals are the door's, not the question's.
    let verdict = Question::Verdict {
        shape: ty(number),
        arguments: &[],
    };
    assert_eq!(asked(verdict), Some(Answer::Verdict(Verdict::Maybe)));
}

#[test]
fn a_binding_is_written_into_a_question_whole_and_in_part() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let symbols = SymbolInterner::new();
    let name = |name| TypeSymbol::declared(name, &symbols).expect("a Type token");
    let (elt, key) = (
        types.lexical(0, name("Elt"), KType::ANY),
        types.lexical(1, name("Key"), KType::ANY),
    );
    let list = types.list(elt);
    let levels = Levels::from_bound(&scratch, &[(0, KType::NUMBER)]);
    let question = Question::Fits {
        a: list.into(),
        b: key.into(),
    };
    let (substituted, changed) = question.substituted(&types, &scratch, &levels);
    assert!(changed);
    assert_eq!(
        substituted,
        Question::Fits {
            a: types.list(Parametric::from(KType::NUMBER)).into(),
            b: key.into(),
        },
        "`Elt` is written in and `Key` is kept"
    );
    let unbound = Levels::from_bound(&scratch, &[(2, KType::NUMBER)]);
    assert_eq!(
        question.substituted(&types, &scratch, &unbound),
        (question, false)
    );
    let whole = Question::Fits {
        a: elt.into(),
        b: ty(KType::NUMBER),
    };
    assert_eq!(
        whole.substituted(&types, &scratch, &levels),
        (
            Question::Fits {
                a: ty(KType::NUMBER),
                b: ty(KType::NUMBER),
            },
            true
        )
    );
}

#[test]
fn a_binding_keeps_an_answer_its_closed_door_gives_alike() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let symbols = SymbolInterner::new();
    let elt = types.lexical(
        0,
        TypeSymbol::declared("Elt", &symbols).unwrap(),
        KType::ANY,
    );
    let answered = Answered {
        question: Question::Fits {
            a: elt.into(),
            b: ty(KType::NUMBER),
        },
        answer: Answer::Bool(true),
    };
    let under = |closed| Levels::from_bound(&scratch, &[(0, closed)]);
    assert!(keeps(&types, &scratch, &answered, &under(KType::NUMBER)));
    assert!(!keeps(&types, &scratch, &answered, &under(KType::STR)));
}

#[test]
fn an_answered_questions_digest_is_alike_across_two_lays() {
    let (a, b) = (Bump::new(), Bump::new());
    let arguments = [Interval::point(KType::NUMBER.into())];
    let answered = Answered {
        question: Question::Verdict {
            shape: ty(KType::STR),
            arguments: &arguments,
        },
        answer: Answer::Verdict(Verdict::Always),
    };
    let (here, there) = (
        resident(&a, answered.laid(&a)),
        resident(&b, answered.laid(&b)),
    );
    assert!(!std::ptr::eq(here, there));
    assert_eq!(here.digest(), there.digest());
    assert_eq!(here, there);
    let never = Answered {
        answer: Answer::Verdict(Verdict::Never),
        ..answered
    };
    assert_ne!(answered.digest(), never.digest());
}

proptest::proptest! {
    /// The closed door answers every closed question, each answer's index lies below the question's
    /// count, and the index names the answer back.
    #[test]
    fn a_closed_question_has_a_counted_answer(
        kind in 0usize..11,
        (a, b, c) in (0usize..12, 0usize..12, 0usize..12),
    ) {
        let (registry, scratch) = (Bump::new(), Bump::new());
        let types = TestTypes::new(&registry).load();
        let union = types.union_of(&scratch, &[KType::NUMBER, KType::STR]);
        let pool = [
            KType::NUMBER,
            KType::STR,
            KType::BOOL,
            KType::NULL,
            KType::ANY,
            KType::NEVER,
            KType::ANY_TYPE,
            KType::ANY_CODE,
            KType::EMPTY_SIGNATURE,
            KType::LIST_OF_ANY,
            types.list(KType::NUMBER),
            union,
        ];
        let (a, b, c) = (pool[a], pool[b], pool[c]);
        let interval = Interval {
            lower: types.union_of(&scratch, &[a, KType::NEVER]).into(),
            upper: types.union_of(&scratch, &[a, c]).into(),
        };
        let arguments = [interval];
        let defined = [ty(a)];
        let question = match kind {
            0 => Question::Verdict { shape: ty(b), arguments: &arguments },
            1 => Question::NeverSatisfies { value: interval, wanted: b.into() },
            2 => Question::Fits { a: ty(a), b: ty(b) },
            3 => Question::LowerEndOutside { value: interval, bound: ty(b) },
            4 => Question::Under { a: a.into(), b: b.into() },
            5 => Question::Same { a: ty(a), b: ty(b) },
            6 => Question::Kind { ty: a.into() },
            7 => Question::Module { ty: a.into() },
            8 => Question::IsFunction { ty: a.into() },
            9 => Question::Defined { rule: super::super::Rule::Fielded, operands: &defined },
            _ => Question::Restamp { declared: a.into() },
        };
        let answered = answer(&types, &scratch, &question).expect("a closed question answers");
        let index = answered.index_in(&question);
        proptest::prop_assert!(index < question.answer_count());
        proptest::prop_assert_eq!(question.nth_answer(index), answered);
    }
}

/// Every open kind is one question's kind, the kind its door marks where no witness answers, or
/// one of the kinds that hold — a `FOR ALL` bound over a run-bound name, a module binder's
/// content, a solve no stored solve stands for — and never both.
#[test]
fn every_kind_but_those_that_hold_is_a_question_s() {
    use super::super::{OpenKind, Rule};
    let number = ty(KType::NUMBER);
    let point = Interval::point(KType::NUMBER.into());
    let one = Parametric::from(KType::NUMBER);
    let questions = [
        Question::Verdict {
            shape: number,
            arguments: &[],
        },
        Question::NeverSatisfies {
            value: point,
            wanted: one,
        },
        Question::Fits {
            a: number,
            b: number,
        },
        Question::LowerEndOutside {
            value: point,
            bound: number,
        },
        Question::Same {
            a: number,
            b: number,
        },
        Question::Kind { ty: one },
        Question::Module { ty: one },
        Question::Restamp { declared: one },
        Question::Under { a: one, b: one },
        Question::Runs {
            kept: &[],
            candidate: 0,
            unfilled: false,
        },
        Question::Arms {
            scrutinee: point,
            clause: None,
            arms: &[],
            arm: 0,
        },
        Question::Overlaps {
            registered: one,
            against: &[],
        },
        Question::Defined {
            rule: Rule::Pinned,
            operands: &[],
        },
        Question::Defined {
            rule: Rule::InstanceAt { variables: 0 },
            operands: &[],
        },
        Question::IsFunction { ty: one },
        Question::Outranks {
            a: number,
            b: number,
            class: 0,
        },
    ];
    let asked: Vec<OpenKind> = questions.iter().map(Question::kind).collect();
    for kind in OpenKind::ALL {
        assert_ne!(
            kind.holds(),
            asked.contains(&kind),
            "{kind:?} is a question's or holds, not both"
        );
    }
}
