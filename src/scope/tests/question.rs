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
        asked(Question::Retype {
            declared: number.into()
        }),
        Some(Answer::Stamps)
    );
    assert_eq!(
        asked(Question::Kind { ty: number.into() }),
        Some(Answer::Kind(KKind::ProperType))
    );
    // A refusal is worded from ends, so it is no answer.
    assert_eq!(
        asked(Question::NeverSatisfies {
            value: Interval::point(number.into()),
            wanted: str.into(),
        }),
        None
    );
    assert_eq!(
        asked(Question::NeverSatisfies {
            value: Interval::point(number.into()),
            wanted: any.into(),
        }),
        Some(Answer::Satisfiable)
    );
    // A verdict is written only where the intervals it hands back are the question's.
    let verdict = |intervals| Question::Verdict {
        shape: ty(number),
        arguments: &[],
        intervals,
    };
    assert_eq!(asked(verdict(None)), Some(Answer::Verdict(Verdict::Maybe)));
    let point = [Interval::point(number.into())];
    assert_eq!(asked(verdict(Some(&point))), None);
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
    let (substituted, changed) =
        question.substituted(&types, &scratch, &mut |level| levels.bound(level));
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
        question.substituted(&types, &scratch, &mut |level| unbound.bound(level)),
        (question, false)
    );
    let whole = Question::Fits {
        a: elt.into(),
        b: ty(KType::NUMBER),
    };
    assert_eq!(
        whole.substituted(&types, &scratch, &mut |level| levels.bound(level)),
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
            intervals: None,
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
