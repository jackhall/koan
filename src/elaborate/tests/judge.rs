//! The judgement doors over names: each marks where a binding decides, and decides where none
//! can.

use std::cell::RefCell;

use crate::elaborate::judge::{self, Marks};
use crate::memory::Bump;
use crate::scope::{DerivedTerm, OpenKind, Question, Rule};
use crate::symbols::TypeSymbol;
use crate::type_lattice::{DeclaredType, KKind, KType, Parametric, Step};
use crate::values::tests::TestTypes;

/// The kinds each judgement left undecided.
#[derive(Default)]
struct Recorded(RefCell<Vec<OpenKind>>);

impl Marks for Recorded {
    fn open(
        &self,
        kind: OpenKind,
        _: Option<DeclaredType<Parametric>>,
        _: DeclaredType<Parametric>,
    ) {
        self.0.borrow_mut().push(kind);
    }
}

/// A union holding a variable may collapse to a member of another kind under a binding, so its
/// kind is the binding's to decide, as a bare variable's is; a closed union's is decided.
#[test]
fn a_union_holding_a_variable_leaves_its_kind_to_the_binding() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let name = TypeSymbol::classify("Elt").expect("a type name");
    let elt = types.lexical(0, name, KType::ANY);
    let union = types.union_of(&scratch, &[elt, KType::NUMBER.into()]);
    let marks = Recorded::default();
    assert_eq!(judge::kind_decided(&marks, &types, &scratch, union), None);
    assert_eq!(*marks.0.borrow(), [OpenKind::Kind]);
    let closed = types.union_of(&scratch, &[KType::STR.into(), KType::NUMBER.into()]);
    let marks = Recorded::default();
    assert_eq!(
        judge::kind_decided(&marks, &types, &scratch, closed),
        Some(KKind::ProperType)
    );
    assert!(marks.0.borrow().is_empty());
}

/// A walk's marks that name every derived name `Str`, recording what each was asked for.
#[derive(Default)]
struct Deriving(RefCell<Vec<Asked>>);

/// What one derived name was asked for: its rule and its output.
type Asked = (Rule, u8);

impl Marks for Deriving {
    fn open(&self, _: OpenKind, _: Option<DeclaredType<Parametric>>, _: DeclaredType<Parametric>) {}

    fn derive(&self, term: DerivedTerm<'_>) -> Option<Parametric> {
        let mut laid = self.0.borrow_mut();
        // Any name but a rule's is recorded as the meet rule's: only its count is read.
        let (rule, output) = match term {
            DerivedTerm::Output { rule, output, .. } => (rule, output),
            _ => (Rule::Meet, u8::MAX),
        };
        laid.push((rule, output));
        Some(Parametric::from(KType::STR))
    }
}

/// A rule over names hands back its closed outputs over closed operands, a piece its bound
/// guarantees as the projection, and otherwise the derived name for its output, whatever arm a
/// binding would take.
#[test]
fn a_rule_over_names_hands_back_its_spelling_or_a_derived_name() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let name = TypeSymbol::classify("Elt").expect("a type name");
    let element = Rule::Piece(Step::Element);
    let marks = Deriving::default();
    let ruled = |operand: Parametric| {
        let operands = [DeclaredType::Type(operand)];
        let ruled = judge::ruled(
            &marks,
            &types,
            &scratch,
            (element, &operands),
            &[KType::ANY.into()],
        );
        ruled.map(|outputs| outputs[0])
    };
    let numbers = types.list(KType::NUMBER);
    assert_eq!(ruled(numbers.into()), Some(KType::NUMBER.into()));
    let listed = types.lexical(0, name, types.list(KType::NUMBER));
    let piece = ruled(listed).expect("a piece the bound guarantees");
    assert_eq!(
        types.projection(&scratch, listed, Step::Element),
        Some(piece)
    );
    assert!(
        marks.0.borrow().is_empty(),
        "nothing derived over a spelled piece"
    );
    let any = types.lexical(0, name, KType::ANY);
    assert_eq!(ruled(any), Some(KType::STR.into()));
    assert_eq!(*marks.0.borrow(), [(element, 0)]);
}

/// A decided type is the one arm where every arm is it, the closed answer's arm over a closed
/// question, and otherwise the derived name typed on the arms, whatever a witness would answer.
#[test]
fn a_decided_type_is_its_arm_where_every_binding_gives_it_else_a_derived_name() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let name = TypeSymbol::classify("Elt").expect("a type name");
    let marks = Deriving::default();
    let never = Parametric::from(KType::NEVER);
    let arrives = |ty: Parametric, arms: &[Parametric]| {
        let question = Question::Same {
            a: DeclaredType::Type(ty),
            b: DeclaredType::Type(never),
        };
        judge::decided(&marks, &types, &scratch, (question, arms), never)
    };
    let number = Parametric::from(KType::NUMBER);
    assert_eq!(arrives(number, &[number, number]), number);
    assert_eq!(arrives(number, &[number, never]), number);
    assert_eq!(arrives(never, &[number, never]), never);
    assert!(
        marks.0.borrow().is_empty(),
        "nothing derived over a decided question"
    );
    let elt = types.lexical(0, name, KType::ANY);
    assert_eq!(arrives(elt, &[number, never]), Parametric::from(KType::STR));
    assert_eq!(marks.0.borrow().len(), 1);
}

/// A signature application keeps its schema's member names at every pin, so a need it lacks a
/// named member of, or a record need, lies outside it at load: wherever the judge says so over a
/// pin written over a name, the lattice says so of the application at every closed pin, and the
/// judge says so wherever the asked names are ones none of its schema declares.
#[test]
fn a_signature_application_over_a_name_lies_outside_what_every_pin_does() {
    use crate::symbols::{BinderSymbol, ValueSymbol};
    use crate::type_lattice::{Interval, SchemaDraft, SigOrigin};
    use crate::values::record_type;

    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let carrier = TypeSymbol::classify("Carrier").expect("a type name");
    let elt = types.lexical(
        0,
        TypeSymbol::classify("Elt").expect("a type name"),
        KType::ANY,
    );
    let names: Vec<ValueSymbol> = ["aa", "bb", "cc"]
        .iter()
        .map(|name| ValueSymbol::classify(name).expect("a value name"))
        .collect();
    let signature = |declared: &[ValueSymbol], origin: SigOrigin, parameter: bool| {
        let mut draft = SchemaDraft::new(&scratch);
        draft.origin = origin;
        if parameter {
            draft.insert_parameter(carrier, types.head_parameter(carrier, KType::ANY));
        }
        for name in declared {
            draft.insert_value_slot(*name, KType::ANY);
        }
        types.signature(&scratch, draft)
    };
    let subsets = |max: usize| (0u8..1 << max).map(move |bits| bits as usize);
    let pins = [KType::NUMBER, KType::STR, KType::NEVER, KType::ANY];
    for declared_bits in subsets(2) {
        let declared: Vec<_> = (0..2)
            .filter(|at| declared_bits & 1 << at != 0)
            .map(|at| names[at])
            .collect();
        let counter = signature(&declared, SigOrigin::Declared, true);
        let apply = |pin: Parametric| {
            types.signature_apply(&scratch, counter, &[(BinderSymbol::Type(carrier), pin)])
        };
        let over_name = apply(elt);
        let mut needs: Vec<(KType, bool)> = Vec::new();
        for asked_bits in subsets(3).filter(|bits| *bits != 0) {
            let asked: Vec<_> = (0..3)
                .filter(|at| asked_bits & 1 << at != 0)
                .map(|at| names[at])
                .collect();
            let lacks = asked.iter().any(|name| !declared.contains(name));
            needs.push((signature(&asked, SigOrigin::Declared, false), lacks));
            let fields = asked
                .iter()
                .map(|name| (BinderSymbol::Value(*name), KType::ANY));
            needs.push((record_type(&types, &scratch, fields), true));
        }
        for (need, lacks) in needs {
            let outside = judge::lower_end_outside(
                &judge::NoMarks,
                &types,
                &scratch,
                Interval::point(over_name),
                need,
            );
            assert_eq!(outside, lacks, "the judge reads the names every pin keeps");
            if !outside {
                continue;
            }
            for pin in pins {
                let closed = Interval::point(apply(pin.into()));
                assert!(
                    crate::type_lattice::lower_end_outside(&types, &scratch, closed, need),
                    "every closed pin lies outside where the names do"
                );
            }
        }
    }
}
