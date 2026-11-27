//! A term handed by hand, and a derived name a binding gives its term handed again: closed where
//! the binding closes it, the name of the term it leaves where it leaves one open, a spelling
//! where the binding makes the term spellable.

use std::cell::RefCell;

use crate::memory::Bump;
use crate::symbols::{SymbolInterner, TypeSymbol};
use crate::type_lattice::{DeclaredType, DerivedId, KType, Parametric, PathClass, Step};
use crate::values::Chain;
use crate::values::tests::TestTypes;

use super::super::question::{Question, rebound};
use super::super::record_types::Closed;
use super::super::rule::Rule;
use super::super::typed::Resolved;
use super::{Binds, Decider, DerivedName, DerivedTerm, Spellings, handed};

/// [`handed`] through a namer that names every term, as a walk's does.
fn named_by(
    types: &Chain<'_, '_, '_, '_>,
    scratch: &Bump,
    term: DerivedTerm<'_>,
    name: &mut dyn FnMut(DerivedTerm<'_>) -> Parametric,
) -> Option<Parametric> {
    let Ok(handed) = handed(types, scratch, term, &mut |term| {
        Ok::<_, std::convert::Infallible>(name(term))
    });
    handed
}

fn name(text: &str, symbols: &SymbolInterner) -> TypeSymbol {
    TypeSymbol::declared(text, symbols).expect("a Type token")
}

/// A test's binding: `levels` by level, each derived name it laid by its id, and each term it
/// meets named and laid.
struct Laying<'x> {
    levels: Vec<(usize, KType)>,
    spellings: Spellings<'x>,
    names: RefCell<Vec<DerivedName<'static>>>,
    scratch: &'static Bump,
}

impl<'x> Laying<'x> {
    fn new(levels: &[(usize, KType)], symbols: &'x SymbolInterner) -> Self {
        Laying {
            levels: levels.to_vec(),
            spellings: Spellings::new(symbols),
            names: RefCell::new(Vec::new()),
            scratch: Box::leak(Box::new(Bump::new())),
        }
    }

    /// The name of `term`, laid.
    fn named(&self, types: &Chain<'_, '_, '_, '_>, term: DerivedTerm<'_>) -> Parametric {
        if let Some(held) = (self.names.borrow().iter()).find(|name| name.id == term.id()) {
            return held.ty;
        }
        let name = self
            .spellings
            .named((types, self.scratch), term.laid(self.scratch));
        self.names.borrow_mut().push(name);
        name.ty
    }
}

impl Binds for Laying<'_> {
    fn level(&self, _: &Chain<'_, '_, '_, '_>, level: usize) -> Option<KType> {
        (self.levels.iter())
            .find(|(held, _)| *held == level)
            .map(|(_, ty)| *ty)
    }

    fn term(&self, id: DerivedId) -> Option<DerivedTerm<'_>> {
        (self.names.borrow().iter())
            .find(|name| name.id == id)
            .map(|name| name.term)
    }

    fn name(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        _: crate::memory::BumpAllocator<'_>,
        term: DerivedTerm<'_>,
    ) -> Option<Parametric> {
        Some(self.named(types, term))
    }
}

/// What `binds` gives `ty`.
fn under(types: &Chain<'_, '_, '_, '_>, ty: Parametric, binds: &dyn Binds) -> Parametric {
    let scratch = Bump::new();
    match rebound(types, &scratch, DeclaredType::Type(ty), binds) {
        Closed::Same => ty,
        Closed::Type(DeclaredType::Type(closed)) => closed.into(),
        Closed::Partial(DeclaredType::Type(partial)) => partial,
        other => panic!("a type closes to a type: {other:?}"),
    }
}

/// A term over closed operands is its rule's closed output; over a variable whose bound does not
/// guarantee the piece, it is the term's name, which the binding of the variable closes to the
/// closed rule's output over the binding.
#[test]
fn a_term_is_its_rule_s_output_over_its_operands_bindings() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let symbols = SymbolInterner::new();
    let elt = types.lexical(0, name("Elt", &symbols), KType::ANY);
    let operands = [DeclaredType::Type(elt)];
    let element = DerivedTerm::Output {
        rule: Rule::Piece(Step::Element),
        operands: &operands,
        output: 0,
    };
    let laying = Laying::new(&[], &symbols);
    let named = named_by(&types, &scratch, element, &mut |term| {
        laying.named(&types, term)
    })
    .expect("a term over a variable is named");
    assert_eq!(laying.names.borrow().len(), 1, "the name is laid");
    let bound = |closed: KType| {
        let binding = Laying::new(&[(0, closed)], &symbols);
        binding
            .names
            .borrow_mut()
            .extend(laying.names.borrow().iter().copied());
        under(&types, named, &binding)
    };
    assert_eq!(bound(types.list(KType::NUMBER)), KType::NUMBER.into());
    // A binding lacking the piece leaves the name: its site refuses apart.
    assert_eq!(bound(KType::NUMBER), named);
    let meet_operands = [
        DeclaredType::Type(elt),
        DeclaredType::Type(KType::NUMBER.into()),
    ];
    let meet = DerivedTerm::Output {
        rule: Rule::Meet,
        operands: &meet_operands,
        output: 0,
    };
    let met = named_by(&types, &scratch, meet, &mut |term| {
        laying.named(&types, term)
    })
    .expect("a meet over a variable is named");
    let either = types.union_of(&scratch, &[KType::NUMBER, KType::STR]);
    let binding = Laying::new(&[(0, either)], &symbols);
    binding
        .names
        .borrow_mut()
        .extend(laying.names.borrow().iter().copied());
    assert_eq!(under(&types, met, &binding), KType::NUMBER.into());
}

/// A binding that leaves a term open gives its name the name of the term it leaves, so every
/// reader under it holds one handle; one that makes the term spellable
/// gives the spelling, as a fresh walk's door hands back.
#[test]
fn a_partly_bound_term_hands_back_what_a_fresh_door_would() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let symbols = SymbolInterner::new();
    let elt = types.lexical(0, name("Elt", &symbols), KType::ANY);
    let key = types.lexical(1, name("Key", &symbols), KType::ANY);
    let operands = [DeclaredType::Type(elt), DeclaredType::Type(key)];
    let meet = DerivedTerm::Output {
        rule: Rule::Meet,
        operands: &operands,
        output: 0,
    };
    let laying = Laying::new(&[], &symbols);
    let named = named_by(&types, &scratch, meet, &mut |term| {
        laying.named(&types, term)
    })
    .expect("a meet over two variables is named");
    // `Elt` bound alone: the name of `(Number & Key)`, as a fresh walk names that term.
    let binding = Laying::new(&[(0, KType::NUMBER)], &symbols);
    binding
        .names
        .borrow_mut()
        .extend(laying.names.borrow().iter().copied());
    let partial = under(&types, named, &binding);
    let fresh_operands = [
        DeclaredType::Type(KType::NUMBER.into()),
        DeclaredType::Type(key),
    ];
    let fresh = DerivedTerm::Output {
        rule: Rule::Meet,
        operands: &fresh_operands,
        output: 0,
    };
    let walk = Laying::new(&[], &symbols);
    let restarted = named_by(&types, &scratch, fresh, &mut |term| {
        walk.named(&types, term)
    });
    assert_eq!(Some(partial), restarted, "one handle for one term");
    // A piece of a variable bound to a list of another: the binding makes it the element.
    let piece_operands = [DeclaredType::Type(elt)];
    let piece = DerivedTerm::Output {
        rule: Rule::Piece(Step::Element),
        operands: &piece_operands,
        output: 0,
    };
    let piece_name = named_by(&types, &scratch, piece, &mut |term| {
        laying.named(&types, term)
    })
    .expect("a piece the bound does not guarantee is named");
    let listed = Laying::new(&[(0, KType::ANY)], &symbols);
    listed
        .names
        .borrow_mut()
        .extend(laying.names.borrow().iter().copied());
    // `Any` lacks the piece: the name stays.
    assert_eq!(under(&types, piece_name, &listed), piece_name);
}

/// A union of variables a binding closes is the closed union of what it gives each: a member
/// closing to `Never` or to one already present drops, as a fresh load's union of the closed types
/// would. A call's result over the candidates it keeps is such a union.
#[test]
fn a_union_of_derived_names_closes_to_the_closed_union() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let symbols = SymbolInterner::new();
    let first = types.lexical(1, name("First", &symbols), KType::ANY);
    let second = types.lexical(2, name("Second", &symbols), KType::ANY);
    let held = types.union_of(&scratch, &[first, second, KType::STR.into()]);
    let closed = |one: KType, two: KType| {
        let binding = Laying::new(&[(1, one), (2, two)], &symbols);
        match rebound(&types, &scratch, DeclaredType::Type(held), &binding) {
            Closed::Type(DeclaredType::Type(closed)) => closed,
            other => panic!("a binding of every level closes the union: {other:?}"),
        }
    };
    let union = |members: &[KType]| types.union_of(&scratch, members);
    assert_eq!(
        closed(KType::NEVER, KType::NUMBER),
        union(&[KType::NUMBER, KType::STR])
    );
    assert_eq!(closed(KType::STR, KType::NEVER), KType::STR);
    assert_eq!(
        closed(KType::NUMBER, KType::NUMBER),
        union(&[KType::NUMBER, KType::STR])
    );
    assert_eq!(closed(KType::NEVER, KType::NEVER), KType::STR);
    let either = union(&[KType::NUMBER, KType::STR]);
    assert_eq!(closed(either, KType::NUMBER), either);
}

/// A decided name is the type on the answer its question takes under the binding, that type
/// closed by the binding too: here a result that arrives only where `Elt` is not `Never`.
#[test]
fn a_decided_name_is_the_type_on_its_question_s_answer() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let symbols = SymbolInterner::new();
    let elt = types.lexical(0, name("Elt", &symbols), KType::ANY);
    let listed = types.list(elt);
    let question = Question::Same {
        a: DeclaredType::Type(elt),
        b: DeclaredType::Type(KType::NEVER.into()),
    };
    // A `Same` question answers no, then yes.
    let arms = [listed, KType::NEVER.into()];
    let arrives = DerivedTerm::Decided {
        decision: Decider::Answer(question),
        arms: &arms,
    };
    let laying = Laying::new(&[], &symbols);
    let named = named_by(&types, &scratch, arrives, &mut |term| {
        laying.named(&types, term)
    })
    .expect("an undecided question names its term");
    let bound = |closed: KType| {
        let binding = Laying::new(&[(0, closed)], &symbols);
        binding
            .names
            .borrow_mut()
            .extend(laying.names.borrow().iter().copied());
        under(&types, named, &binding)
    };
    assert_eq!(bound(KType::NUMBER), types.list(KType::NUMBER).into());
    assert_eq!(bound(KType::NEVER), KType::NEVER.into());
    let (lower, bound) = arrives.ends(&types, &scratch);
    assert_eq!((lower, bound), (KType::NEVER, types.list(KType::ANY)));
}

/// A binding that moves the paths of one class to another: what a binding written into the
/// application a class's paths read through makes of them.
struct Renaming {
    from: PathClass,
    to: Resolved,
}

impl Binds for Renaming {
    fn level(&self, _: &Chain<'_, '_, '_, '_>, _: usize) -> Option<KType> {
        None
    }

    fn class(
        &self,
        _: &Chain<'_, '_, '_, '_>,
        _: crate::memory::BumpAllocator<'_>,
        class: PathClass,
    ) -> Option<Resolved> {
        (class == self.from).then_some(self.to)
    }
}

/// A path variable a binding moves to another class is that class's variable, of the same name
/// and bound, wherever it sits; one the binding closes is the closed type.
#[test]
fn a_binding_moves_a_path_variable_to_its_class_or_closes_it() {
    let registry = Bump::new();
    let types = TestTypes::new(&registry).load();
    let symbols = SymbolInterner::new();
    let right = name("Right", &symbols);
    let (from, to) = (PathClass([1, 0]), PathClass([2, 0]));
    let held = types.path_variable(from, right, KType::ANY);
    let list = |ty: Parametric| types.list(ty);
    let moved = Renaming {
        from,
        to: Resolved::Class(to),
    };
    let renamed = types.path_variable(to, right, KType::ANY);
    assert_eq!(under(&types, held, &moved), renamed);
    assert_eq!(under(&types, list(held), &moved), list(renamed));
    let closed = Renaming {
        from,
        to: Resolved::Closed(KType::STR),
    };
    assert_eq!(
        under(&types, list(held), &closed),
        types.list(KType::STR).into()
    );
}
