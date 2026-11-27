//! The questions a door asks: the closed door's answer per kind, a binding written into a question
//! whole and in part, whether a binding keeps an answer, and an answered question's digest.

use crate::memory::Bump;
use crate::symbols::{SymbolInterner, TypeSymbol};
use crate::type_lattice::{DeclaredType, Interval, KKind, KType, Parametric, Verdict};
use crate::values::tests::TestTypes;

use super::super::{Answer, ArmChoice, ArmsLayout, Levels, Question, Selecting, Selection, answer};

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
            a: ty(number).into(),
            b: ty(any)
        }),
        Some(Answer::Bool(true))
    );
    assert_eq!(
        asked(Question::Fits {
            a: ty(any).into(),
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
        shape: ty(number).into(),
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
                a: ty(KType::NUMBER).into(),
                b: ty(KType::NUMBER),
            },
            true
        )
    );
}

/// A closer writing in the closed types `levels` binds, as a store's binding of levels does.
struct LevelCloser<'q, 'l> {
    types: &'q crate::values::Chain<'q, 'q, 'q, 'q>,
    lay: &'l Bump,
    levels: &'q Levels<'q>,
}

impl crate::scope::Closer for LevelCloser<'_, '_> {
    fn close(&mut self, ty: DeclaredType<Parametric>) -> crate::scope::Closed {
        crate::scope::rebound(self.types, self.lay, ty, self.levels)
    }

    fn concrete(&self, ty: Parametric) -> Option<KType> {
        self.types.concrete(ty)
    }

    fn rebuilt(
        &mut self,
        scheme: crate::type_lattice::Scheme,
        rebounds: crate::scope::Rebounds<'_>,
    ) -> Option<DeclaredType<KType>> {
        let (types, scratch) = (self.types, self.lay);
        crate::scope::rebuilt_closed(types, scratch, scheme, rebounds, &mut |ty| self.close(ty))
    }
}

impl<'l> crate::scope::Laying<'l> for LevelCloser<'_, 'l> {
    type Lay = &'l Bump;

    fn lay(&self) -> &'l Bump {
        self.lay
    }
}

/// An instance over rigid variables closes once the binding closes every point, a point closed
/// already among them: a binding leaves a closed point as it is, which is closed all the same.
#[test]
fn an_instance_closes_where_a_point_was_closed_already() {
    use crate::scope::{CloseLaying, StaticInstance};
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let symbols = SymbolInterner::new();
    let elt = types.lexical(
        0,
        TypeSymbol::declared("Elt", &symbols).unwrap(),
        KType::ANY,
    );
    let rigid = StaticInstance::Rigid {
        solution: scratch.alloc_slice_copy(&[Parametric::from(KType::NUMBER), elt]),
        ktype: types.list(elt),
        variables: &[],
    };
    let levels = Levels::from_bound(&scratch, &[(0, KType::STR)]);
    let mut closer = LevelCloser {
        types: &types,
        lay: &scratch,
        levels: &levels,
    };
    match rigid.closed_laying(&mut closer) {
        StaticInstance::Closed { solution, ktype } => {
            assert_eq!(solution, &[KType::NUMBER, KType::STR]);
            assert_eq!(ktype, types.list(KType::STR));
        }
        StaticInstance::Rigid { .. } => panic!("every point closes"),
    }
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
            0 => Question::Verdict { shape: ty(b).into(), arguments: &arguments },
            1 => Question::NeverSatisfies { value: interval, wanted: b.into() },
            2 => Question::Fits { a: ty(a).into(), b: ty(b) },
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
            shape: number.into(),
            arguments: &[],
        },
        Question::NeverSatisfies {
            value: point,
            wanted: one,
        },
        Question::Fits {
            a: number.into(),
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
            selection: Selection {
                asks: &[],
                layout: Selecting {
                    listed: 0,
                    single: false,
                    arriving: 0,
                    open: 0,
                    ranked: 0,
                    classes: 0,
                    ranks: 0,
                    refusals: 1,
                    refused: None,
                    dropped: None,
                    members: 0,
                    agrees: 0,
                    split: None,
                },
                candidates: &[],
            },
            candidate: 0,
        },
        Question::Arms {
            choice: ArmChoice {
                asks: &[],
                layout: ArmsLayout {
                    count: 0,
                    arriving: 0,
                    ranks: 0,
                    ranked: 0,
                },
                guards: &[],
            },
            arm: 0,
        },
        Question::Overlaps {
            registered: one,
            against: &[],
        },
        Question::Defined {
            rule: Rule::Fielded,
            operands: &[],
        },
        Question::Defined {
            rule: Rule::InstanceAt {
                variables: 0,
                bounded: 0,
            },
            operands: &[],
        },
        Question::IsFunction { ty: one },
        Question::Outranks {
            a: number.into(),
            b: number.into(),
            class: 0,
        },
    ];
    let asked: Vec<OpenKind> = questions.iter().map(Question::kind).collect();
    for kind in OpenKind::ALL {
        assert!(asked.contains(&kind), "{kind:?} is a question's");
    }
}
