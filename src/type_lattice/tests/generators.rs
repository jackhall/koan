//! The bespoke strategy the laws run over: type trees interned into a live registry as they are
//! built, so every generated value is a real handle rather than a description of one.
//!
//! The alphabets are tiny on purpose — three binder names, three Type names, two keywords, two
//! value names — because the interesting collisions are structural, and a wide alphabet makes two
//! generated types share a shape only by accident.
//!
//! The registry and every scratch buffer live in the bump tier — storage outside the graph — so a
//! test owns a bare arena. A strategy is `'static`, so the registry it interns into lives over an
//! arena leaked for the rest of the test process. Everything transient — a generated value's
//! scratch buffers, a sealed group's window — lives in a fresh arena dropped as soon as the value
//! is built ([`with_scratch`]), so generation keeps nothing but interned content.

use std::collections::HashMap;
use std::rc::Rc;

use proptest::prelude::*;

use crate::memory::{Bump, BumpAllocator, ScopeId};
use crate::parse::{BinderSymbol, KeywordSymbol, LabelInterner, TypeSymbol, ValueSymbol};

use crate::type_lattice::handle::KType;
use crate::type_lattice::kind::KKind;
use crate::type_lattice::operators::{FoldDirection, ReductionMode};
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::schema::SchemaDraft;
use crate::type_lattice::shape::{DeferredReturnSurface, DispatchTokenElement};
use crate::type_lattice::window::{RecursiveGroupWindow, RelativeSchema};

/// An arena that lives for the rest of the test process, so a strategy can hold handles into it.
fn leaked_arena() -> BumpAllocator<'static> {
    Box::leak(Box::new(Bump::new()))
}

/// Run `build` over a fresh scratch arena, dropped as soon as it returns.
pub fn with_scratch<R>(build: impl FnOnce(BumpAllocator<'_>) -> R) -> R {
    build(&Bump::new())
}

/// One registry, one interner and the alphabets, shared by every strategy in a `proptest!` block.
#[derive(Clone)]
pub struct World {
    pub types: Rc<TypeRegistry<'static>>,
    pub labels: Rc<LabelInterner>,
    pub binders: Rc<Vec<BinderSymbol>>,
    pub type_names: Rc<Vec<TypeSymbol>>,
    pub keywords: Rc<Vec<KeywordSymbol>>,
    pub values: Rc<Vec<ValueSymbol>>,
}

impl World {
    pub fn new() -> World {
        let labels = Rc::new(LabelInterner::new());
        let declare_binder =
            |text: &str| BinderSymbol::declared(text, &labels).expect("a bindable token");
        let declare_type = |text: &str| TypeSymbol::declared(text, &labels).expect("a Type token");
        let declare_keyword =
            |text: &str| KeywordSymbol::declared(text, &labels).expect("a keyword token");
        let declare_value =
            |text: &str| ValueSymbol::declared(text, &labels).expect("a value token");
        World {
            types: Rc::new(TypeRegistry::in_region(leaked_arena())),
            binders: Rc::new(vec![
                declare_binder("x"),
                declare_binder("y"),
                declare_binder("z"),
            ]),
            type_names: Rc::new(vec![
                declare_type("Elt"),
                declare_type("Item"),
                declare_type("Wrap"),
            ]),
            keywords: Rc::new(vec![declare_keyword("PURE"), declare_keyword("WRAP")]),
            values: Rc::new(vec![declare_value("a"), declare_value("b")]),
            labels,
        }
    }

    /// Types with no free variable in them — what a rigid variable's bound is drawn from, so
    /// nothing above a bound is itself rigid.
    fn grounds(&self) -> Vec<KType> {
        vec![
            KType::NUMBER,
            KType::STR,
            KType::ANY,
            KType::LIST_OF_ANY,
            KType::of_kind(KKind::ProperType),
        ]
    }
}

impl Default for World {
    fn default() -> Self {
        World::new()
    }
}

/// A type tree of at most `depth` composite levels, interned as it is built.
pub fn arb_type(world: World, depth: u32) -> BoxedStrategy<KType> {
    arb_type_in(world, depth, Rc::new(Vec::new()), Rc::new(Vec::new()))
}

/// [`arb_type`] with the rigid variables in scope: `bound` are an enclosing shape's quantifiers,
/// which do not cross into a nested shape's own binder, and `members` are an enclosing signature's
/// abstract members, which do.
fn arb_type_in(
    world: World,
    depth: u32,
    bound: Rc<Vec<KType>>,
    members: Rc<Vec<KType>>,
) -> BoxedStrategy<KType> {
    let leaf = arb_leaf(world.clone(), bound.clone(), members.clone());
    if depth == 0 {
        return leaf;
    }
    let inner = || arb_type_in(world.clone(), depth - 1, bound.clone(), members.clone());
    let (list_world, dict_world) = (world.clone(), world.clone());
    let (record_world, function_world) = (world.clone(), world.clone());
    let (union_world, apply_world) = (world.clone(), world.clone());
    let (shape_world, sig_world, group_world) = (world.clone(), world.clone(), world.clone());
    let shape_members = members.clone();
    prop_oneof![
        6 => leaf,
        2 => inner().prop_map(move |element| list_world.types.list(element)),
        2 => (inner(), inner())
            .prop_map(move |(key, value)| dict_world.types.dict(key, value)),
        2 => arb_fields(record_world.clone(), inner()).prop_map(move |fields| {
            with_scratch(|scratch| record_world.types.record(scratch, &fields))
        }),
        2 => (arb_fields(function_world.clone(), inner()), inner()).prop_map(
            move |(params, ret)| {
                with_scratch(|scratch| function_world.types.function_type(scratch, &params, ret))
            }
        ),
        2 => prop::collection::vec(inner(), 1..4).prop_map(move |members| {
            with_scratch(|scratch| union_world.types.union_of(scratch, &members))
        }),
        1 => (inner(), arb_fields(apply_world.clone(), inner())).prop_map(
            move |(constructor, arguments)| {
                with_scratch(|scratch| {
                    apply_world
                        .types
                        .constructor_apply(scratch, constructor, &arguments)
                })
            }
        ),
        3 => arb_shape(shape_world, depth, shape_members),
        2 => arb_signature(sig_world, depth),
        1 => arb_sealed_member(group_world, depth),
    ]
    .boxed()
}

/// The atoms, plus whatever rigid variables are in scope.
///
/// A free abstract leaf is minted **generative** — an opaque ascription's per-application mint —
/// so it is never mistaken for one of an enclosing signature's own members. A schema's members
/// reach a slot only by being handed down through `members`, which is the invariant projection
/// guarantees: a nonce-free abstract sourced at the canonical binder *is* a declared member.
fn arb_leaf(world: World, bound: Rc<Vec<KType>>, members: Rc<Vec<KType>>) -> BoxedStrategy<KType> {
    let grounds = world.grounds();
    let abstract_world = world.clone();
    let atoms = prop_oneof![
        Just(KType::NUMBER),
        Just(KType::STR),
        Just(KType::BOOL),
        Just(KType::ANY),
        Just(KType::NEVER),
        Just(KType::IDENTIFIER),
        Just(KType::NAME_TOKEN),
        Just(KType::TYPE_NAME_TOKEN),
        Just(KType::KEXPRESSION),
        Just(KType::of_kind(KKind::ProperType)),
        Just(KType::of_kind(KKind::AnyType)),
        Just(KType::of_kind(KKind::NewType)),
    ];
    let deferred = {
        let names = world.type_names.clone();
        let types = world.types.clone();
        (0..names.len())
            .prop_map(move |index| types.deferred_return(DeferredReturnSurface::Type(names[index])))
            .boxed()
    };
    let opaque = (
        0..abstract_world.type_names.len(),
        0..3usize,
        0..grounds.len(),
    )
        .prop_map(move |(name, arity, bound)| {
            let params = &abstract_world.type_names[..arity.min(2)];
            with_scratch(|scratch| {
                abstract_world.types.abstract_type(
                    scratch,
                    OPAQUE_MINT,
                    abstract_world.type_names[name],
                    params,
                    Some(OPAQUE_MINT),
                    abstract_world.grounds()[bound],
                )
            })
        });
    let mut rigid: Vec<KType> = bound.as_ref().clone();
    rigid.extend(members.iter().copied());
    if rigid.is_empty() {
        return prop_oneof![8 => atoms, 1 => deferred, 3 => opaque].boxed();
    }
    let in_scope = (0..rigid.len()).prop_map(move |index| rigid[index]);
    prop_oneof![6 => atoms, 1 => deferred, 2 => opaque, 4 => in_scope].boxed()
}

/// The declaring scope a generated opaque mint is sourced at — any id that is not the canonical
/// binder, since the canonical binder is reserved for a signature's own members.
const OPAQUE_MINT: ScopeId = ScopeId::from_raw(1, 1);

/// Zero to three named fields over `value`, keyed from the binder alphabet, each name once: a
/// later draw for a name replaces an earlier one in place, the record a parser that rejects
/// duplicate fields would have kept.
fn arb_fields(
    world: World,
    value: BoxedStrategy<KType>,
) -> impl Strategy<Value = Vec<(BinderSymbol, KType)>> + use<> {
    prop::collection::vec((0..world.binders.len(), value), 0..3).prop_map(move |drawn| {
        let mut fields: Vec<(BinderSymbol, KType)> = Vec::new();
        for (name, kt) in drawn {
            let name = world.binders[name];
            match fields.iter_mut().find(|(held, _)| *held == name) {
                Some(field) => field.1 = kt,
                None => fields.push((name, kt)),
            }
        }
        fields
    })
}

/// An expression shape, sometimes over a quantifier group of its own. Every variable is minted
/// under a variable-free bound.
///
/// A variable is planted at **two** argument positions on purpose: canonical form replaces a single
/// occurrence by its bound or by `Never`, so a group whose variables were only sprinkled at random
/// would almost never survive interning, and the laws about quantified shapes would run over
/// nothing.
fn arb_shape(world: World, depth: u32, members: Rc<Vec<KType>>) -> BoxedStrategy<KType> {
    let grounds = world.grounds();
    (
        1..4usize,
        prop::collection::vec(0..grounds.len(), 0..2),
        prop::collection::vec((0..4usize, 0..4usize), 0..2),
    )
        .prop_flat_map(move |(positions, bounds, plantings)| {
            let world = world.clone();
            let vars: Rc<Vec<KType>> = Rc::new(
                bounds
                    .iter()
                    .enumerate()
                    .map(|(index, bound)| world.types.quantified(index, world.grounds()[*bound]))
                    .collect(),
            );
            let names: Vec<TypeSymbol> = world.type_names[..bounds.len()].to_vec();
            let positions = positions.max(bounds.len() * 2).min(4);
            let slot = arb_type_in(
                world.clone(),
                depth.saturating_sub(1),
                vars.clone(),
                members.clone(),
            );
            let ret = arb_type_in(
                world.clone(),
                depth.saturating_sub(1),
                vars.clone(),
                members.clone(),
            );
            (
                prop::collection::vec((0..world.keywords.len(), slot), positions),
                ret,
            )
                .prop_map(move |(drawn, ret)| {
                    let mut keywords: Vec<KeywordSymbol> = Vec::new();
                    let mut slots: Vec<KType> = Vec::new();
                    for (keyword, slot) in drawn {
                        keywords.push(world.keywords[keyword]);
                        slots.push(slot);
                    }
                    for (index, variable) in vars.iter().enumerate() {
                        let (first, second) = plantings.get(index).copied().unwrap_or((0, 1));
                        let (first, second) = (first % slots.len(), second % slots.len());
                        if first == second {
                            continue;
                        }
                        slots[first] = *variable;
                        slots[second] = *variable;
                    }
                    let mut run: Vec<DispatchTokenElement> = Vec::new();
                    for (keyword, slot) in keywords.into_iter().zip(slots) {
                        run.push(DispatchTokenElement::Keyword(keyword));
                        run.push(DispatchTokenElement::Slot(slot));
                    }
                    with_scratch(|scratch| {
                        world.types.shape_type(scratch, &names, &run, ret).handle
                    })
                })
        })
        .boxed()
}

/// An interface with a handful of members of each kind.
///
/// The abstract members are chosen first and their handles handed down to every member type, so a
/// slot that names `Elt` names the very handle the schema binds it to — which is what projection
/// produces and what the relations assume.
fn arb_signature(world: World, depth: u32) -> BoxedStrategy<KType> {
    let grounds = world.grounds();
    let outer = world.clone();
    prop::collection::vec((0..world.type_names.len(), 0..grounds.len()), 0..2)
        .prop_flat_map(move |declared| {
            let world = outer.clone();
            let mut abstract_members: Vec<(TypeSymbol, KType)> = Vec::new();
            for (name, bound) in declared {
                let name = world.type_names[name];
                if abstract_members.iter().all(|(held, _)| *held != name) {
                    let member = with_scratch(|scratch| {
                        world.types.abstract_type(
                            scratch,
                            ScopeId::SENTINEL,
                            name,
                            &[],
                            None,
                            world.grounds()[bound],
                        )
                    });
                    abstract_members.push((name, member));
                }
            }
            let members: Rc<Vec<KType>> =
                Rc::new(abstract_members.iter().map(|(_, kt)| *kt).collect());
            let none = Rc::new(Vec::new());
            let manifest = arb_type_in(world.clone(), depth - 1, none.clone(), members.clone());
            let slot = arb_type_in(world.clone(), depth - 1, none.clone(), members.clone());
            let keyworded = arb_shape(world.clone(), depth.clamp(1, 2), members.clone());
            (
                prop::collection::vec((0..world.type_names.len(), manifest), 0..2),
                prop::collection::vec((0..world.values.len(), slot), 0..2),
                prop::collection::vec(keyworded, 0..2),
                prop::collection::vec((0..world.keywords.len(), 0..3usize), 0..2),
            )
                .prop_map(move |(manifests, slots, keyworded, operators)| {
                    with_scratch(|scratch| {
                        let mut draft = SchemaDraft::new(scratch);
                        for (name, kt) in manifests {
                            let name = world.type_names[name];
                            if abstract_members.iter().all(|(held, _)| *held != name) {
                                draft.insert_manifest(name, kt);
                            }
                        }
                        for (name, kt) in slots {
                            draft.insert_value_slot(world.values[name], kt);
                        }
                        draft.sig_id = (!abstract_members.is_empty()).then_some(ScopeId::SENTINEL);
                        for (name, member) in &abstract_members {
                            draft.insert_abstract(*name, *member);
                        }
                        for shape in keyworded {
                            draft.push_keyworded(shape);
                        }
                        let mut claimed: HashMap<KeywordSymbol, ReductionMode> = HashMap::new();
                        for (keyword, mode) in operators {
                            let keyword = world.keywords[keyword];
                            let mode = match mode {
                                0 => ReductionMode::FoldLeft,
                                1 => ReductionMode::FoldRight,
                                _ => ReductionMode::Pairwise {
                                    combiner: world.keywords[0],
                                    direction: FoldDirection::Left,
                                },
                            };
                            // A channel never has two records over one operator, which the
                            // generator has to respect: the invariant is the schema's, not the
                            // relation's.
                            if claimed.insert(keyword, mode).is_none() {
                                draft.push_operator_group(&[keyword], mode);
                            }
                        }
                        world.types.signature(scratch, draft)
                    })
                })
        })
        .boxed()
}

/// One member of a freshly sealed recursive group, whose relative schemas may hold unions over
/// sibling references — the shape the seal's canonicalization claim is about. The window is hosted
/// in the value's own scratch region, which it does not outlive.
fn arb_sealed_member(world: World, depth: u32) -> BoxedStrategy<KType> {
    let repr = arb_type_in(
        world.clone(),
        depth.saturating_sub(1),
        Rc::new(Vec::new()),
        Rc::new(Vec::new()),
    );
    (
        1..4usize,
        prop::collection::vec((repr, any::<bool>()), 1..4),
    )
        .prop_map(move |(count, reprs)| {
            let count = count.min(reprs.len()).max(1);
            let names: Vec<TypeSymbol> = world.type_names[..count.min(3)].to_vec();
            let announced: Vec<(TypeSymbol, KKind)> =
                names.iter().map(|name| (*name, KKind::NewType)).collect();
            with_scratch(|scratch| {
                let window = RecursiveGroupWindow::new(scratch, &announced);
                for index in 0..names.len() {
                    let (repr, recursive) = reprs[index];
                    let body = if recursive {
                        let sibling = window.sibling(
                            names[(index + 1) % names.len()],
                            KKind::NewType,
                            &world.types,
                        );
                        world.types.union_of(scratch, &[repr, sibling])
                    } else {
                        repr
                    };
                    window.fill_member(index, RelativeSchema::NewType(body), &world.types, scratch);
                }
                window
                    .sealed()
                    .and_then(|sealed| sealed.member(0))
                    .expect("the window seals on its last fill")
            })
        })
        .boxed()
}

/// A generated expression shape, for the laws whose subject is a shape and which a draw from the
/// whole vocabulary would leave mostly vacuous.
pub fn arb_shape_type(world: World, depth: u32) -> BoxedStrategy<KType> {
    arb_shape(world, depth, Rc::new(Vec::new()))
}

/// A tuple of argument types for a shape of `arity` positions, drawn from the ground alphabet plus a
/// couple of composites — what the admission laws feed a candidate.
pub fn arb_arguments(world: World, arity: usize) -> impl Strategy<Value = Vec<KType>> + use<> {
    let pool = {
        let mut pool = world.grounds();
        pool.push(KType::BOOL);
        pool.push(KType::NEVER);
        pool.push(with_scratch(|scratch| {
            world.types.union_of(scratch, &[KType::NUMBER, KType::STR])
        }));
        pool
    };
    prop::collection::vec(0..pool.len(), arity)
        .prop_map(move |picks| picks.into_iter().map(|index| pool[index]).collect())
}
