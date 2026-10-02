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
//!
//! Generation runs over raw [`Handle`]s, as the lattice itself does, and each public strategy hands
//! its draw out typed: [`arb_concrete`] a [`KType`], every other one a [`DeclaredType`], since a
//! draw may be a scheme.

use std::collections::HashMap;
use std::rc::Rc;

use proptest::prelude::*;

use crate::memory::{Bump, BumpAllocator, ScopeId};
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner, TypeSymbol, ValueSymbol};

use crate::type_lattice::handle::{
    DeclaredType, Handle, KType, Parametric, Scheme, TypeHandle, wrap,
};
use crate::type_lattice::kind::KKind;
use crate::type_lattice::node::TypeNode;
use crate::type_lattice::operators::{FoldDirection, ReductionMode};
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::schema::{SchemaDraft, SigOrigin};
use crate::type_lattice::shape::{
    DeferredReturnSurface, DispatchTokenElement, RawRank, dense_classes,
};
use crate::type_lattice::typed::{instantiate_quantified, join, meet, quantifier_bounds};
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
    pub symbols: Rc<SymbolInterner>,
    pub binders: Rc<Vec<BinderSymbol>>,
    pub type_names: Rc<Vec<TypeSymbol>>,
    pub keywords: Rc<Vec<KeywordSymbol>>,
    pub values: Rc<Vec<ValueSymbol>>,
}

impl World {
    pub fn new() -> World {
        let symbols = Rc::new(SymbolInterner::new());
        let declare_binder =
            |text: &str| BinderSymbol::declared(text, &symbols).expect("a bindable token");
        let declare_type = |text: &str| TypeSymbol::declared(text, &symbols).expect("a Type token");
        let declare_keyword =
            |text: &str| KeywordSymbol::declared(text, &symbols).expect("a keyword token");
        let declare_value =
            |text: &str| ValueSymbol::declared(text, &symbols).expect("a value token");
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
            symbols,
        }
    }

    /// Types with no free variable in them — what a rigid variable's bound is drawn from, so
    /// nothing above a bound is itself rigid.
    fn grounds(&self) -> Vec<KType> {
        vec![
            KType::NUMBER,
            KType::STR,
            KType::ANY,
            KType::ANY_VALUE,
            KType::ANY_CODE,
            KType::of_kind(KKind::AnyType),
            KType::LIST_OF_ANY,
            KType::of_kind(KKind::ProperType),
            // A bound spanning two members, which the order, a union's canonical form, the meet
            // and the unifier each read whole.
            with_scratch(|scratch| self.types.union_of(scratch, &[KType::NUMBER, KType::STR])),
        ]
    }

    /// A raw draw as what it is declared as: a scheme, or a type that may hold a variable.
    pub fn declared(&self, raw: Handle) -> DeclaredType<Parametric> {
        self.types.declared(raw)
    }

    /// A raw draw from a concrete strategy as the [`KType`] it is.
    fn concrete(&self, raw: Handle) -> KType {
        self.types
            .concrete(wrap(raw))
            .expect("a concrete strategy draws a concrete type")
    }
}

impl Default for World {
    fn default() -> Self {
        World::new()
    }
}

/// A type tree of at most `depth` composite levels, interned as it is built: any type, parametric
/// ones and schemes included.
pub fn arb_any(world: World, depth: u32) -> BoxedStrategy<DeclaredType<Parametric>> {
    arb_raw(world.clone(), depth)
        .prop_map(move |raw| world.declared(raw))
        .boxed()
}

/// [`arb_any`]'s draw, raw.
fn arb_raw(world: World, depth: u32) -> BoxedStrategy<Handle> {
    arb_type_in(
        world,
        depth,
        Rc::new(Vec::new()),
        Rc::new(Vec::new()),
        false,
    )
}

/// A **concrete** type tree: no free `Quantified`, lexical variable, head parameter or quantified
/// binder outside sealed content. Opaque carriers, signatures, sealed families and their
/// applications are drawn as [`arb_any`] draws them.
pub fn arb_concrete(world: World, depth: u32) -> BoxedStrategy<KType> {
    arb_type_in(
        world.clone(),
        depth,
        Rc::new(Vec::new()),
        Rc::new(Vec::new()),
        true,
    )
    .prop_map(move |raw| world.concrete(raw))
    .boxed()
}

/// [`arb_type`] with the rigid variables in scope: `bound` are an enclosing shape's quantifiers,
/// which do not cross into a nested shape's own binder, and `members` are an enclosing signature's
/// head parameters, which do. `concrete` draws no variable and no binder outside sealed content.
fn arb_type_in(
    world: World,
    depth: u32,
    bound: Rc<Vec<Handle>>,
    members: Rc<Vec<Handle>>,
    concrete: bool,
) -> BoxedStrategy<Handle> {
    let leaf = arb_leaf(world.clone(), bound.clone(), members.clone(), concrete);
    if depth == 0 {
        return leaf;
    }
    let inner = || {
        arb_type_in(
            world.clone(),
            depth - 1,
            bound.clone(),
            members.clone(),
            concrete,
        )
    };
    let (list_world, dict_world) = (world.clone(), world.clone());
    let (record_world, function_world) = (world.clone(), world.clone());
    let (union_world, apply_world) = (world.clone(), world.clone());
    let (shape_world, sig_world, group_world) = (world.clone(), world.clone(), world.clone());
    let (family_world, applied_world) = (world.clone(), world.clone());
    let (shape_members, function_members) = (members.clone(), members.clone());
    // A constructor is a concrete type, so an application draws one.
    let constructor = arb_type_in(
        world.clone(),
        depth - 1,
        bound.clone(),
        members.clone(),
        true,
    );
    prop_oneof![
        6 => leaf,
        2 => inner().prop_map(move |element| list_world.types.list(element)),
        2 => (inner(), inner())
            .prop_map(move |(key, value)| dict_world.types.dict(key, value)),
        2 => arb_fields(record_world.clone(), inner()).prop_map(move |fields| {
            with_scratch(|scratch| record_world.types.record(scratch, &fields))
        }),
        2 => arb_function(function_world, depth, function_members, concrete),
        2 => prop::collection::vec(inner(), 1..4).prop_map(move |members| {
            with_scratch(|scratch| union_world.types.union_of(scratch, &members))
        }),
        1 => (constructor, arb_fields(apply_world.clone(), inner())).prop_map(
            move |(constructor, arguments)| {
                let constructor = apply_world.concrete(constructor);
                with_scratch(|scratch| {
                    apply_world
                        .types
                        .constructor_apply(scratch, constructor, &arguments)
                })
            }
        ),
        3 => arb_shape(shape_world, depth, shape_members, concrete),
        2 => arb_signature_type(sig_world, depth).prop_map(KType::raw),
        1 => arb_sealed_member(group_world, depth).prop_map(KType::raw),
        1 => arb_family(family_world.clone(), depth).prop_map(KType::raw),
        1 => (arb_family(family_world, depth), inner()).prop_map(move |(family, argument)| {
            let parameter = BinderSymbol::Type(applied_world.type_names[0]);
            with_scratch(|scratch| {
                applied_world
                    .types
                    .constructor_apply(scratch, family, &[(parameter, argument)])
            })
        }),
    ]
    .boxed()
}

/// The atoms, plus whatever rigid variables are in scope.
///
/// A free abstract leaf is minted **generative** — an opaque ascription's per-application mint —
/// so it is never mistaken for one of an enclosing signature's own members. A schema's members
/// reach a slot only by being handed down through `members`, which is the invariant projection
/// guarantees: a nonce-free abstract sourced at the canonical binder *is* a declared member.
fn arb_leaf(
    world: World,
    bound: Rc<Vec<Handle>>,
    members: Rc<Vec<Handle>>,
    concrete: bool,
) -> BoxedStrategy<Handle> {
    let grounds = world.grounds();
    let abstract_world = world.clone();
    let atoms = prop_oneof![
        Just(KType::NUMBER.raw()),
        Just(KType::STR.raw()),
        Just(KType::BOOL.raw()),
        Just(Handle::ANY),
        Just(Handle::ANY_VALUE),
        Just(Handle::ANY_CODE),
        Just(Handle::NEVER),
        Just(Handle::IDENTIFIER),
        Just(Handle::SYMBOL),
        Just(Handle::TYPE_NAME_TOKEN),
        Just(Handle::EXPRESSION),
        Just(Handle::LITERAL),
        Just(Handle::BLOCK),
        Just(Handle::DECLARATION),
        Just(Handle::BINDER),
        Just(Handle::NAME),
        Just(Handle::KEYWORD),
        Just(KType::of_kind(KKind::ProperType).raw()),
        Just(KType::of_kind(KKind::AnyType).raw()),
        Just(KType::of_kind(KKind::NewType).raw()),
    ];
    let deferred = {
        let names = world.type_names.clone();
        let types = world.types.clone();
        (0..names.len())
            .prop_map(move |index| {
                types
                    .deferred_return(DeferredReturnSurface::Type(names[index]))
                    .raw()
            })
            .boxed()
    };
    let opaque =
        (0..abstract_world.type_names.len(), 0..grounds.len()).prop_map(move |(name, bound)| {
            abstract_world
                .types
                .carrier(
                    abstract_world.type_names[name],
                    abstract_world.grounds()[bound],
                    OPAQUE_MINT,
                )
                .raw()
        });
    // A lexical variable: one of two levels, named from the type alphabet, over a ground bound —
    // and over the `Number | Str` ground, sometimes above a lower end of one of its members.
    let lexical_world = world.clone();
    let lexical = (
        0..2usize,
        0..world.type_names.len(),
        0..grounds.len(),
        0..3u8,
    )
        .prop_map(move |(level, name, bound, lower)| {
            let (types, name) = (&lexical_world.types, lexical_world.type_names[name]);
            let bound = lexical_world.grounds()[bound];
            let spans = matches!(types.node(bound), TypeNode::Union { .. });
            let variable = match lower {
                1 if spans => with_scratch(|scratch| {
                    types.lexical_between(scratch, level, name, KType::NUMBER, bound)
                }),
                2 if spans => with_scratch(|scratch| {
                    types.lexical_between(scratch, level, name, KType::STR, bound)
                }),
                _ => types.lexical(level, name, bound),
            };
            variable.raw()
        });
    // A code kind needing names: a kind from three levels of the code tree, needing one to three
    // names of the binder alphabet.
    let needing_world = world.clone();
    let needing = (
        prop::sample::select(&[KType::BLOCK, KType::EXPRESSION, KType::BINDER][..]),
        prop::sample::subsequence(world.binders.as_ref().clone(), 1..=3),
    )
        .prop_map(move |(kind, names)| {
            with_scratch(|scratch| {
                needing_world
                    .types
                    .code_needing(scratch, kind, &names)
                    .raw()
            })
        });
    if concrete {
        return prop_oneof![8 => atoms, 1 => deferred, 3 => opaque, 2 => needing].boxed();
    }
    let mut rigid: Vec<Handle> = bound.as_ref().clone();
    rigid.extend(members.iter().copied());
    if rigid.is_empty() {
        return prop_oneof![8 => atoms, 1 => deferred, 3 => opaque, 2 => lexical, 2 => needing]
            .boxed();
    }
    let in_scope = (0..rigid.len()).prop_map(move |index| rigid[index]);
    prop_oneof![
        6 => atoms,
        1 => deferred,
        2 => opaque,
        2 => lexical,
        2 => needing,
        4 => in_scope
    ]
    .boxed()
}

/// The declaring scope a generated opaque mint is sourced at — any id that is not the canonical
/// binder, since the canonical binder is reserved for a signature's own members.
const OPAQUE_MINT: ScopeId = ScopeId::from_raw(1, 1);

/// Zero to three named fields over `value`, keyed from the binder alphabet, each name once: a
/// later draw for a name replaces an earlier one in place, the record a parser that rejects
/// duplicate fields would have kept.
fn arb_fields(
    world: World,
    value: BoxedStrategy<Handle>,
) -> impl Strategy<Value = Vec<(BinderSymbol, Handle)>> + use<> {
    prop::collection::vec((0..world.binders.len(), value), 0..3).prop_map(move |drawn| {
        let mut fields: Vec<(BinderSymbol, Handle)> = Vec::new();
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

/// An expression shape, sometimes over a quantifier group of its own — never under `concrete`.
/// Every variable is minted under a variable-free bound.
///
/// A variable is planted at **two** argument positions on purpose, so the laws about quantified
/// shapes run over variables that relate two positions, which a group sprinkled at random would
/// rarely give. Each slot is ranked `_` or by a small integer, so written order and rankings with ties
/// both occur. Both occurrences take one [`planted`] form, so a union argument can pour its
/// members into one variable through a list or a function's parameter.
fn arb_shape(
    world: World,
    depth: u32,
    members: Rc<Vec<Handle>>,
    concrete: bool,
) -> BoxedStrategy<Handle> {
    let grounds = world.grounds();
    let arity = if concrete { 0..1 } else { 0..2 };
    (
        1..4usize,
        prop::collection::vec(0..grounds.len(), arity),
        prop::collection::vec((0..4usize, 0..4usize, 0..3u8), 0..2),
    )
        .prop_flat_map(move |(positions, bounds, plantings)| {
            let world = world.clone();
            let vars: Rc<Vec<Handle>> = Rc::new(
                bounds
                    .iter()
                    .enumerate()
                    .map(|(index, bound)| {
                        world.types.quantified(index, world.grounds()[*bound]).raw()
                    })
                    .collect(),
            );
            let names: Vec<TypeSymbol> = world.type_names[..bounds.len()].to_vec();
            let bounds: Vec<KType> = bounds.iter().map(|bound| world.grounds()[*bound]).collect();
            let positions = positions.max(bounds.len() * 2).min(4);
            let slot = arb_type_in(
                world.clone(),
                depth.saturating_sub(1),
                vars.clone(),
                members.clone(),
                concrete,
            );
            let ret = arb_type_in(
                world.clone(),
                depth.saturating_sub(1),
                vars.clone(),
                members.clone(),
                concrete,
            );
            (
                prop::collection::vec((0..world.keywords.len(), slot), positions),
                ret,
                prop::collection::vec(prop::option::of(0..3u32), positions),
            )
                .prop_map(move |(drawn, ret, ranks)| {
                    let mut keywords: Vec<KeywordSymbol> = Vec::new();
                    let mut slots: Vec<Handle> = Vec::new();
                    for (keyword, slot) in drawn {
                        keywords.push(world.keywords[keyword]);
                        slots.push(slot);
                    }
                    for (index, variable) in vars.iter().enumerate() {
                        let (first, second, form) =
                            plantings.get(index).copied().unwrap_or((0, 1, 0));
                        let (first, second) = (first % slots.len(), second % slots.len());
                        if first == second {
                            continue;
                        }
                        let planted = planted(&world, *variable, form);
                        slots[first] = planted;
                        slots[second] = planted;
                    }
                    let mut run: Vec<DispatchTokenElement> = Vec::new();
                    for (keyword, slot) in keywords.into_iter().zip(slots) {
                        run.push(DispatchTokenElement::Keyword(keyword));
                        run.push(DispatchTokenElement::Slot(slot));
                    }
                    let ranks: Vec<RawRank> = ranks
                        .into_iter()
                        .map(|rank| rank.map_or(RawRank::Unnumbered, RawRank::Numbered))
                        .collect();
                    with_scratch(|scratch| {
                        let classes = dense_classes(scratch, &ranks);
                        world
                            .types
                            .shape_group(scratch, &names, &bounds, &run, classes, ret)
                            .0
                    })
                })
        })
        .boxed()
}

/// A function type, sometimes over a quantifier group of its own — never under `concrete`. Every
/// variable is minted under a variable-free bound.
///
/// A variable is planted at **two** positions — two parameters, or a parameter and the return —
/// for the reason [`arb_shape`] plants one at two slots. Both occurrences take one [`planted`]
/// form.
fn arb_function(
    world: World,
    depth: u32,
    members: Rc<Vec<Handle>>,
    concrete: bool,
) -> BoxedStrategy<Handle> {
    let grounds = world.grounds();
    let arity = if concrete { 0..1 } else { 0..2 };
    (
        1..4usize,
        prop::collection::vec(0..grounds.len(), arity),
        prop::collection::vec((0..4usize, 0..4usize, 0..3u8), 0..2),
    )
        .prop_flat_map(move |(arity, bounds, plantings)| {
            let world = world.clone();
            let vars: Rc<Vec<Handle>> = Rc::new(
                bounds
                    .iter()
                    .enumerate()
                    .map(|(index, bound)| {
                        world.types.quantified(index, world.grounds()[*bound]).raw()
                    })
                    .collect(),
            );
            let names: Vec<TypeSymbol> = world.type_names[..bounds.len()].to_vec();
            let bounds: Vec<KType> = bounds.iter().map(|bound| world.grounds()[*bound]).collect();
            let arity = arity.min(world.binders.len());
            let value = arb_type_in(
                world.clone(),
                depth.saturating_sub(1),
                vars.clone(),
                members.clone(),
                concrete,
            );
            let ret = arb_type_in(
                world.clone(),
                depth.saturating_sub(1),
                vars.clone(),
                members.clone(),
                concrete,
            );
            (prop::collection::vec(value, arity), ret).prop_map(move |(drawn, ret)| {
                // The return is the last plantable position, so even a one-parameter function
                // gives a variable two places to occur.
                let mut positions = drawn;
                positions.push(ret);
                for (index, variable) in vars.iter().enumerate() {
                    let (first, second, form) = plantings.get(index).copied().unwrap_or((0, 1, 0));
                    let (first, second) = (first % positions.len(), second % positions.len());
                    if first == second {
                        continue;
                    }
                    let planted = planted(&world, *variable, form);
                    positions[first] = planted;
                    positions[second] = planted;
                }
                let ret = positions.pop().expect("the return is the last position");
                let params: Vec<(BinderSymbol, Handle)> =
                    world.binders.iter().copied().zip(positions).collect();
                with_scratch(|scratch| {
                    world
                        .types
                        .function_group(scratch, &names, &bounds, &params, ret)
                        .0
                })
            })
        })
        .boxed()
}

/// A planted variable in one of three forms: itself (`form` 0), a list's element (1), or a
/// function's parameter (2), which reaches the variable from above.
fn planted(world: &World, variable: Handle, form: u8) -> Handle {
    match form {
        0 => variable,
        1 => world.types.list(variable),
        _ => function_of(world, variable),
    }
}

/// `FN :{x :parameter} -> Null`.
fn function_of(world: &World, parameter: Handle) -> Handle {
    with_scratch(|scratch| {
        world
            .types
            .function_type(scratch, &[(world.binders[0], parameter)], KType::NULL.raw())
    })
}

/// A signature type: a signature, an application of one pinning some of its head parameters at
/// ground types, or the meet of two applications.
fn arb_signature_type(world: World, depth: u32) -> BoxedStrategy<KType> {
    let meet_world = world.clone();
    prop_oneof![
        2 => arb_signature(world.clone(), depth),
        2 => arb_application(world.clone(), depth),
        1 => (arb_application(world.clone(), depth), arb_application(world, depth)).prop_map(
            move |(a, b)| with_scratch(|scratch| meet_world.types.signature_meet(scratch, &[a, b]))
        ),
    ]
    .boxed()
}

/// A drawn signature with each of its head parameters pinned at a ground type or left open — the
/// signature itself where it pins none.
fn arb_application(world: World, depth: u32) -> BoxedStrategy<KType> {
    let grounds = world.grounds();
    let picks = prop::collection::vec(prop::option::of(0..grounds.len()), 2);
    (arb_signature(world.clone(), depth), picks)
        .prop_map(move |(signature, picks)| {
            let TypeNode::Signature { schema, .. } = world.types.node(signature) else {
                unreachable!("a drawn signature is one");
            };
            let pins: Vec<(BinderSymbol, KType)> = schema
                .parameters
                .iter()
                .zip(picks)
                .filter_map(|((name, _), pick)| {
                    pick.map(|pick| (BinderSymbol::Type(*name), grounds[pick]))
                })
                .collect();
            with_scratch(|scratch| world.types.signature_apply(scratch, signature, &pins))
        })
        .boxed()
}

/// An interface with a handful of members of each kind: a declared signature over head parameters
/// with ground bounds, or a module's schema, which has none.
///
/// The parameters are chosen first and their handles handed down to every member type, so a slot
/// that names `Elt` names the very handle the schema binds it to — which is what the elaborator
/// produces and what the relations assume.
fn arb_signature(world: World, depth: u32) -> BoxedStrategy<KType> {
    let grounds = world.grounds();
    let outer = world.clone();
    (
        prop::collection::vec((0..world.type_names.len(), 0..grounds.len()), 0..2),
        any::<bool>(),
    )
        .prop_flat_map(move |(declared, module)| {
            let world = outer.clone();
            let mut parameters: Vec<(TypeSymbol, Parametric)> = Vec::new();
            for (name, bound) in declared {
                let name = world.type_names[name];
                if parameters.iter().all(|(held, _)| *held != name) {
                    let parameter = world.types.head_parameter(name, world.grounds()[bound]);
                    parameters.push((name, parameter));
                }
            }
            let origin = if module && parameters.is_empty() {
                SigOrigin::Module
            } else {
                SigOrigin::Declared
            };
            let members: Rc<Vec<Handle>> =
                Rc::new(parameters.iter().map(|(_, kt)| kt.raw()).collect());
            let none = Rc::new(Vec::new());
            let manifest = arb_type_in(
                world.clone(),
                depth - 1,
                none.clone(),
                members.clone(),
                false,
            );
            let slot = arb_type_in(
                world.clone(),
                depth - 1,
                none.clone(),
                members.clone(),
                false,
            );
            let keyworded = arb_shape(world.clone(), depth.clamp(1, 2), members.clone(), false);
            (
                prop::collection::vec((0..world.type_names.len(), manifest), 0..2),
                prop::collection::vec((0..world.values.len(), slot), 0..2),
                prop::collection::vec(keyworded, 0..2),
                prop::collection::vec((0..world.keywords.len(), 0..3usize), 0..2),
            )
                .prop_map(move |(manifests, slots, keyworded, operators)| {
                    with_scratch(|scratch| {
                        let mut draft = SchemaDraft::new(scratch);
                        // A manifest member is a type: a scheme drawn for one is left out.
                        for (name, kt) in manifests {
                            let name = world.type_names[name];
                            let DeclaredType::Type(kt) = world.declared(kt) else {
                                continue;
                            };
                            if parameters.iter().all(|(held, _)| *held != name) {
                                draft.insert_manifest(name, kt);
                            }
                        }
                        for (name, kt) in slots {
                            draft.insert_value_slot(world.values[name], world.declared(kt));
                        }
                        draft.origin = origin;
                        for (name, parameter) in &parameters {
                            draft.insert_parameter(*name, *parameter);
                        }
                        for shape in keyworded {
                            draft.push_keyworded(world.declared(shape));
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
    let repr = arb_concrete(world.clone(), depth.saturating_sub(1));
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

/// A freshly sealed one-parameter family, whose representation is a generated type unioned with
/// its parameter's quantifier, so the parameter occurs. The quantifier sits at a covariant
/// position only, as a declared family's must.
fn arb_family(world: World, depth: u32) -> BoxedStrategy<KType> {
    arb_raw(world.clone(), depth.saturating_sub(1))
        .prop_map(move |repr| {
            let (parameter, name) = (world.type_names[0], world.type_names[2]);
            with_scratch(|scratch| {
                let quantifier = world.types.quantified(0, KType::ANY).raw();
                let body = world.types.union_of(scratch, &[repr, quantifier]);
                let body = world
                    .declared(body)
                    .as_type()
                    .expect("a union with a quantifier binds no group");
                let window = RecursiveGroupWindow::new(scratch, &[(name, KKind::TypeConstructor)]);
                let schema =
                    RelativeSchema::constructor(scratch, scratch, Some(body), &[parameter]);
                window
                    .fill_member(0, schema, &world.types, scratch)
                    .and_then(|sealed| sealed.member(0))
                    .expect("a singleton window seals on its fill")
            })
        })
        .boxed()
}

/// A generated expression shape, for the laws whose subject is a shape and which a draw from the
/// whole vocabulary would leave mostly vacuous.
pub fn arb_shape_type(world: World, depth: u32) -> BoxedStrategy<DeclaredType<Parametric>> {
    arb_shape(world.clone(), depth, Rc::new(Vec::new()), false)
        .prop_map(move |raw| world.declared(raw))
        .boxed()
}

/// A generated function type, for the laws whose subject is a function and which a draw from the
/// whole vocabulary would leave mostly vacuous.
pub fn arb_function_type(world: World, depth: u32) -> BoxedStrategy<DeclaredType<Parametric>> {
    arb_function(world.clone(), depth, Rc::new(Vec::new()), false)
        .prop_map(move |raw| world.declared(raw))
        .boxed()
}

/// A function scheme and a function type it may be wanted at: an unrelated function type, or the
/// scheme's own instance at drawn ground bindings, under which an instance always lies. An
/// unrelated draw alone almost never meets its scheme.
pub fn arb_wanted_instance(world: World, depth: u32) -> BoxedStrategy<(Scheme, Parametric)> {
    let grounds = world.grounds();
    let scheme = arb_function(world.clone(), depth, Rc::new(Vec::new()), false).prop_filter_map(
        "a scheme",
        {
            let world = world.clone();
            move |raw| world.declared(raw).as_scheme()
        },
    );
    let unrelated = arb_function(world.clone(), depth, Rc::new(Vec::new()), false).prop_filter_map(
        "an unquantified function type",
        {
            let world = world.clone();
            move |raw| world.declared(raw).as_type()
        },
    );
    (
        scheme,
        unrelated,
        prop::collection::vec(0..grounds.len(), 3),
        any::<bool>(),
    )
        .prop_map(move |(scheme, unrelated, picks, own)| {
            if !own {
                return (scheme, unrelated);
            }
            let bindings: Vec<KType> = picks.iter().map(|pick| grounds[*pick]).collect();
            let instance = with_scratch(|scratch| {
                let bindings = &bindings[..quantifier_bounds(&world.types, scheme).len()];
                instantiate_quantified(&world.types, scratch, scheme, bindings)
            });
            (scheme, instance)
        })
        .boxed()
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
        // Unions whose members pour into one planted variable, through a list or a parameter.
        let lists = [
            world.types.list(KType::NUMBER),
            world.types.list(KType::STR),
        ];
        pool.push(with_scratch(|scratch| {
            world.types.union_of(scratch, &lists)
        }));
        let functions = [
            world.concrete(function_of(&world, KType::NUMBER.raw())),
            world.concrete(function_of(&world, KType::STR.raw())),
        ];
        pool.push(with_scratch(|scratch| {
            world.types.union_of(scratch, &functions)
        }));
        pool
    };
    prop::collection::vec(0..pool.len(), arity)
        .prop_map(move |picks| picks.into_iter().map(|index| pool[index]).collect())
}

/// A chain `a ≤ b ≤ c` through an instance, which [`arb_type`]'s draws almost never line up: `a` is
/// a binder over one variable at two positions, `b` its instance at the least instance `c`'s two
/// positions give that variable, and `c` puts two ground types at those positions.
///
/// The binder is a function, or a shape with both slots in one class. A position is the variable
/// itself — two ground types join into it — or `FN :{x :_} -> Null` over it, where they meet. An
/// optional filler position holds one generated type in all three.
pub fn arb_instance_chain(
    world: World,
) -> BoxedStrategy<(
    DeclaredType<Parametric>,
    DeclaredType<Parametric>,
    DeclaredType<Parametric>,
)> {
    let pool = {
        let mut pool = world.grounds();
        pool.push(KType::BOOL);
        pool.push(world.types.list(KType::NUMBER));
        pool.push(world.types.list(KType::STR));
        pool
    };
    let filler = prop::option::of(arb_raw(world.clone(), 1));
    (
        any::<bool>(),
        0..pool.len(),
        0..pool.len(),
        any::<bool>(),
        filler,
    )
        .prop_map(move |(shape, g1, g2, wrapped, filler)| {
            let (g1, g2) = (pool[g1], pool[g2]);
            let position = |kt| if wrapped { function_of(&world, kt) } else { kt };
            let variable = world.types.quantified(0, KType::ANY).raw();
            let instance = with_scratch(|scratch| {
                if wrapped {
                    meet(&world.types, scratch, g1, g2)
                } else {
                    join(&world.types, scratch, g1, g2)
                }
            });
            let (g1, g2, instance) = (g1.raw(), g2.raw(), instance.raw());
            let build = |quantifiers: &[TypeSymbol], first: Handle, second: Handle| {
                with_scratch(|scratch| {
                    if shape {
                        let (pure, wrap) = (world.keywords[0], world.keywords[1]);
                        let mut run = vec![
                            DispatchTokenElement::Keyword(pure),
                            DispatchTokenElement::Slot(first),
                            DispatchTokenElement::Keyword(wrap),
                            DispatchTokenElement::Slot(second),
                        ];
                        if let Some(filler) = filler {
                            run.push(DispatchTokenElement::Keyword(pure));
                            run.push(DispatchTokenElement::Slot(filler));
                        }
                        let slots = run.len() / 2;
                        let ranks = vec![RawRank::Numbered(1); slots];
                        let classes = dense_classes(scratch, &ranks);
                        world
                            .types
                            .shape_group(
                                scratch,
                                quantifiers,
                                &[KType::ANY][..quantifiers.len()],
                                &run,
                                classes,
                                KType::NULL.raw(),
                            )
                            .0
                    } else {
                        let mut params =
                            vec![(world.binders[1], first), (world.binders[2], second)];
                        if let Some(filler) = filler {
                            params.push((world.binders[0], filler));
                        }
                        world
                            .types
                            .function_group(
                                scratch,
                                quantifiers,
                                &[KType::ANY][..quantifiers.len()],
                                &params,
                                KType::NULL.raw(),
                            )
                            .0
                    }
                })
            };
            let a = build(
                &world.type_names[..1],
                position(variable),
                position(variable),
            );
            let b = build(&[], position(instance), position(instance));
            let c = build(&[], position(g1), position(g2));
            (world.declared(a), world.declared(b), world.declared(c))
        })
        .boxed()
}
