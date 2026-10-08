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
//! its draw out at the narrowest type it guarantees: a [`KType`] where the draw is concrete, a
//! [`Parametric`] for a variable, a [`Scheme`] for a binder, and a [`DeclaredType`] where it may be
//! either.
//!
//! A law draws its precondition by construction rather than filtering for it, since a case that
//! passes without exercising the law is invisible to proptest. A [`Vocabulary`] says what a draw
//! may hold — variables, and whether a shape or function binds a group of its own — and the paired
//! strategies build what independent draws line up only by accident: two shapes over one key
//! ([`arb_shape_pair`]), arguments a candidate admits ([`arb_argument_pair`]), a shape below
//! another ([`arb_shape_below`]), an ordered pair or chain by [`Widening`]s ([`arb_chain`]), and a
//! scheme with its own instance ([`arb_own_instance`]). What still misses a precondition is a
//! `prop_assume!` the laws' reject budget counts.

use std::collections::HashMap;
use std::rc::Rc;

use proptest::prelude::*;

use crate::bump::{Bump, BumpAllocator};
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner, TypeSymbol, ValueSymbol};

use crate::types::handle::{DeclaredType, Handle, KType, Parametric, Scheme, TypeHandle, wrap};
use crate::types::kind::KKind;
use crate::types::node::{ContentKey, TypeNode};
use crate::types::operators::{FoldDirection, ReductionMode};
use crate::types::registry::TypeRegistry;
use crate::types::schema::{SchemaDraft, SigOrigin};
use crate::types::shape::shape_slots;
use crate::types::shape::{DeferredReturnSurface, DispatchTokenElement, RawRank, dense_classes};
use crate::types::typed::{instantiate_quantified, is_subtype_of, join, meet, quantifier_bounds};
use crate::types::walk::Variance;
use crate::types::walk::unary::{Visit, visit};
use crate::types::window::{RecursiveGroupWindow, RelativeSchema};
use crate::types::{lattice, substitute};

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

    /// What a quantifier is bound to: the argument pool, plus a list and a function alone — a
    /// ground, a composite or `Never`, and a lexical variable over a union among them has a member
    /// for its lower end.
    fn binding_pool(&self) -> Vec<KType> {
        let mut pool = self.argument_pool();
        pool.push(self.types.list(KType::NUMBER));
        pool.push(self.concrete(function_of(self, KType::NUMBER.raw())));
        pool
    }

    /// The `pick`th binding-pool type within `bound`, counting round, `Never` left out where
    /// `inhabited`: `Never` lies within every bound and a ground bound within itself, so one does
    /// either way.
    fn binding_within(
        &self,
        scratch: BumpAllocator<'_>,
        bound: KType,
        pick: usize,
        inhabited: bool,
    ) -> KType {
        let within: Vec<KType> = self
            .binding_pool()
            .into_iter()
            .filter(|kt| !(inhabited && *kt == KType::NEVER))
            .filter(|kt| is_subtype_of(&self.types, scratch, *kt, bound))
            .collect();
        within[pick % within.len()]
    }

    /// `binding` within `bound`, for the `index`th variable of a group: its type, or where `lexical`
    /// the lexical variable over that type — an inhabited one, since a variable over `Never` would
    /// be its point. The drawn name is shifted by `index`, so a group's lexical bindings are named
    /// apart: a scope binds a name once at a level.
    fn bind_within(
        &self,
        scratch: BumpAllocator<'_>,
        bound: KType,
        index: usize,
        binding: &Binding,
        lexical: bool,
    ) -> Handle {
        let within = self.binding_within(scratch, bound, binding.within, lexical);
        if !lexical {
            return within.raw();
        }
        let name = self.type_names[(binding.name + index) % self.type_names.len()];
        lexical_over(self, scratch, binding.level, name, within, binding.lower).raw()
    }

    /// The index of the ground spanning two members among [`grounds`](World::grounds).
    fn spanning_ground(&self) -> usize {
        self.grounds()
            .iter()
            .position(|kt| matches!(self.types.node(*kt), TypeNode::Union { .. }))
            .expect("one ground is a union")
    }

    /// The grounds plus a few composites: what an argument is drawn from, and what a widening
    /// unions in.
    fn argument_pool(&self) -> Vec<KType> {
        let mut pool = self.grounds();
        pool.push(KType::BOOL);
        pool.push(KType::NEVER);
        pool.push(with_scratch(|scratch| {
            self.types.union_of(scratch, &[KType::NUMBER, KType::STR])
        }));
        // Unions whose members pour into one planted variable, through a list or a parameter.
        let lists = [self.types.list(KType::NUMBER), self.types.list(KType::STR)];
        pool.push(with_scratch(|scratch| self.types.union_of(scratch, &lists)));
        let functions = [
            self.concrete(function_of(self, KType::NUMBER.raw())),
            self.concrete(function_of(self, KType::STR.raw())),
        ];
        pool.push(with_scratch(|scratch| {
            self.types.union_of(scratch, &functions)
        }));
        pool
    }

    /// What a widening unions in: the argument pool, less `Never`, which widens nothing, and `Any`,
    /// which is the top step.
    fn widening_pool(&self) -> Vec<KType> {
        self.argument_pool()
            .into_iter()
            .filter(|kt| *kt != KType::NEVER && *kt != KType::ANY)
            .collect()
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

/// What a draw may contain beyond the atoms and composites.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Vocabulary {
    /// Rigid variables outside sealed content: lexical variables and whatever an enclosing shape's
    /// group or signature's head hands down.
    pub variables: bool,
    /// Whether a shape or function may (or must) bind a quantifier group of its own.
    pub groups: Groups,
}

/// Whether a drawn shape or function binds a quantifier group of its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Groups {
    Never,
    Maybe,
    /// The outermost shape or function binds one; what lies under it may.
    Always,
    /// The outermost shape or function binds one; nothing under it does.
    Outer,
}

impl Vocabulary {
    /// No variable and no binder: a concrete type.
    pub const CONCRETE: Vocabulary = Vocabulary {
        variables: false,
        groups: Groups::Never,
    };
    /// Everything.
    pub const ANY: Vocabulary = Vocabulary {
        variables: true,
        groups: Groups::Maybe,
    };
    /// Variables but no binder anywhere: a type, never a scheme.
    pub const CLOSED: Vocabulary = Vocabulary {
        variables: true,
        groups: Groups::Never,
    };

    /// What the positions under the outermost shape or function draw: `Always` and `Outer` bind the
    /// outermost group only.
    fn nested(self) -> Vocabulary {
        let groups = match self.groups {
            Groups::Always => Groups::Maybe,
            Groups::Outer => Groups::Never,
            groups => groups,
        };
        Vocabulary { groups, ..self }
    }

    /// The group sizes a binder over `positions` plantable positions may draw: each variable takes
    /// two of them, so a group has up to `positions / 2`.
    fn group_sizes(self, positions: usize) -> std::ops::Range<usize> {
        let most = positions / 2;
        match self.groups {
            Groups::Never => 0..1,
            Groups::Maybe => 0..most + 1,
            Groups::Always | Groups::Outer => {
                assert!(
                    most >= 1,
                    "a binder that must bind a group has two positions"
                );
                1..most + 1
            }
        }
    }
}

/// A type tree of at most `depth` composite levels, interned as it is built: any type, parametric
/// ones and schemes included.
pub fn arb_any(world: World, depth: u32) -> BoxedStrategy<DeclaredType<Parametric>> {
    arb_raw(world.clone(), depth)
        .prop_map(move |raw| world.declared(raw))
        .boxed()
}

/// [`arb_any`] under `vocabulary`, which may not demand a group: a mixed draw has no one outermost
/// binder to demand it of.
pub fn arb_any_with(
    world: World,
    depth: u32,
    vocabulary: Vocabulary,
) -> BoxedStrategy<DeclaredType<Parametric>> {
    let none = || Rc::new(Vec::new());
    arb_type_in(world.clone(), depth, none(), none(), vocabulary)
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
        Vocabulary::ANY,
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
        Vocabulary::CONCRETE,
    )
    .prop_map(move |raw| world.concrete(raw))
    .boxed()
}

/// [`arb_any`] with the rigid variables in scope: `bound` are an enclosing shape's quantifiers,
/// which do not cross into a nested shape's own binder, and `members` are an enclosing signature's
/// head parameters, which do. `vocabulary` says what may occur outside sealed content; a group it
/// demands belongs to a shape or function drawn directly, not to this mixed draw.
fn arb_type_in(
    world: World,
    depth: u32,
    bound: Rc<Vec<Handle>>,
    members: Rc<Vec<Handle>>,
    vocabulary: Vocabulary,
) -> BoxedStrategy<Handle> {
    debug_assert!(matches!(vocabulary.groups, Groups::Never | Groups::Maybe));
    let leaf = arb_leaf(world.clone(), bound.clone(), members.clone(), vocabulary);
    if depth == 0 {
        return leaf;
    }
    let inner = || {
        arb_type_in(
            world.clone(),
            depth - 1,
            bound.clone(),
            members.clone(),
            vocabulary,
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
        Vocabulary::CONCRETE,
    );
    prop_oneof![
        6 => leaf,
        2 => inner().prop_map(move |element| list_world.types.list(element)),
        2 => (inner(), inner())
            .prop_map(move |(key, value)| dict_world.types.dict(key, value)),
        2 => arb_fields(record_world.clone(), inner()).prop_map(move |fields| {
            with_scratch(|scratch| record_world.types.record(scratch, &fields))
        }),
        2 => arb_function(function_world, depth, function_members, vocabulary),
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
        3 => arb_shape(shape_world, depth, shape_members, vocabulary),
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
    vocabulary: Vocabulary,
) -> BoxedStrategy<Handle> {
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
    let opaque = arb_opaque(world.clone()).prop_map(KType::raw);
    let lexical = arb_lexical(world.clone()).prop_map(|variable| variable.raw());
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
    if !vocabulary.variables {
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

/// A head parameter, named from the type alphabet over a ground bound, beside a type that may read
/// it anywhere a signature's member type may.
pub fn arb_over_head_parameter(world: World, depth: u32) -> BoxedStrategy<(TypeSymbol, Handle)> {
    (0..world.type_names.len(), 0..world.grounds().len())
        .prop_flat_map(move |(name, bound)| {
            let name = world.type_names[name];
            let parameter = world.types.head_parameter(name, world.grounds()[bound]);
            let members = Rc::new(vec![parameter.raw()]);
            arb_type_in(
                world.clone(),
                depth,
                Rc::new(Vec::new()),
                members,
                Vocabulary::ANY,
            )
            .prop_map(move |kt| (name, kt))
        })
        .boxed()
}

/// An opaque carrier, named from the type alphabet, meeting a ground bound.
pub fn arb_opaque(world: World) -> BoxedStrategy<KType> {
    (0..world.type_names.len(), 0..world.grounds().len())
        .prop_map(move |(name, bound)| {
            let bound = world.grounds()[bound];
            world
                .types
                .carrier(world.type_names[name], bound, OPAQUE_KEY)
        })
        .boxed()
}

/// A lexical variable: one of two levels, named from the type alphabet, over a ground bound —
/// weighted toward the `Number | Str` ground, where it may sit above a lower end of one member.
pub fn arb_lexical(world: World) -> BoxedStrategy<Parametric> {
    let grounds = world.grounds().len();
    let bound = prop_oneof![2 => Just(world.spanning_ground()), 1 => 0..grounds];
    (0..2usize, 0..world.type_names.len(), bound, 0..3u8)
        .prop_map(move |(level, name, bound, lower)| {
            let (name, bound) = (world.type_names[name], world.grounds()[bound]);
            with_scratch(|scratch| lexical_over(&world, scratch, level, name, bound, lower))
        })
        .boxed()
}

/// The lexical variable at `level` named `name` over `bound`, above a lower end of one member —
/// the first for `lower` 1, the second for 2 — where `bound` is a union.
fn lexical_over(
    world: &World,
    scratch: BumpAllocator<'_>,
    level: usize,
    name: TypeSymbol,
    bound: KType,
    lower: u8,
) -> Parametric {
    let types = &world.types;
    match (types.node(bound), lower) {
        (TypeNode::Union { members }, 1 | 2) => {
            let member = members
                .get(usize::from(lower) - 1)
                .expect("a union has two members");
            types.lexical_between(scratch, level, name, member, bound)
        }
        _ => types.lexical(level, name, bound),
    }
}

/// One quantifier's binding, drawn before its bound is known: a binding-pool type within the
/// bound, or the lexical variable over that type at a level and name, with a lower end where the
/// type is a union. Which of the two is the drawing strategy's to say.
#[derive(Clone, Debug)]
pub struct Binding {
    within: usize,
    level: usize,
    name: usize,
    lower: u8,
}

/// A [`Binding`] over the world's alphabets.
fn arb_binding(world: &World) -> impl Strategy<Value = Binding> + use<> {
    (
        0..world.binding_pool().len(),
        0..2usize,
        0..world.type_names.len(),
        0..3u8,
    )
        .prop_map(|(within, level, name, lower)| Binding {
            within,
            level,
            name,
            lower,
        })
}

/// Whether a quantified binder — a shape or function over a group of its own — is reachable from
/// `kt`.
fn holds_binder(types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>, kt: Handle) -> bool {
    visit(types, scratch, kt, &mut |_, node, _| {
        if node.binds_quantifiers() {
            Visit::Stop
        } else {
            Visit::Descend
        }
    })
}

/// The declaring scope a generated opaque mint is sourced at — any id that is not the canonical
/// binder, since the canonical binder is reserved for a signature's own members.
const OPAQUE_KEY: ContentKey = ContentKey(1);

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

/// A bucket key: a run of keywords from the keyword alphabet, `positions` long.
fn arb_key(
    world: World,
    positions: std::ops::Range<usize>,
) -> impl Strategy<Value = Vec<KeywordSymbol>> + use<> {
    prop::collection::vec(0..world.keywords.len(), positions)
        .prop_map(move |picks| picks.into_iter().map(|k| world.keywords[k]).collect())
}

/// An expression shape over a key of one to four keywords — see [`arb_shape_over`].
fn arb_shape(
    world: World,
    depth: u32,
    members: Rc<Vec<Handle>>,
    vocabulary: Vocabulary,
) -> BoxedStrategy<Handle> {
    arb_key(world.clone(), 1..5)
        .prop_flat_map(move |key| {
            arb_shape_over(world.clone(), key, depth, members.clone(), vocabulary)
        })
        .boxed()
}

/// An expression shape over `key`, over a quantifier group of its own where `vocabulary` allows or
/// demands one. Every variable is minted under a variable-free bound.
///
/// Each variable is planted at **two** positions on purpose — two slots, or a slot and the return,
/// so a one-slot shape binds a group too — so the laws about quantified shapes run over variables
/// that relate two positions, which a group sprinkled at random would rarely give; the group has
/// at most `(key.len() + 1) / 2` variables, so no planting overwrites another. Each slot is ranked
/// `_` or by a small integer, so written order and rankings with ties both occur. Each occurrence
/// takes a [`planted`] form, both one form half the time, so a union argument can pour its members
/// into one variable through a list or a function's parameter, and a function's parameter can cap
/// what a bare occurrence pours in.
fn arb_shape_over(
    world: World,
    key: Vec<KeywordSymbol>,
    depth: u32,
    members: Rc<Vec<Handle>>,
    vocabulary: Vocabulary,
) -> BoxedStrategy<Handle> {
    let grounds = world.grounds();
    let positions = key.len();
    // The return is the last plantable position, as in `arb_function`.
    prop::collection::vec(0..grounds.len(), vocabulary.group_sizes(positions + 1))
        .prop_flat_map(move |bounds| {
            let world = world.clone();
            let key = key.clone();
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
            let position = || {
                arb_type_in(
                    world.clone(),
                    depth.saturating_sub(1),
                    vars.clone(),
                    members.clone(),
                    vocabulary.nested(),
                )
            };
            (
                prop::collection::vec(position(), positions + 1),
                prop::collection::vec(prop::option::of(0..3u32), positions),
                arb_plantings(positions + 1, vars.len()),
            )
                .prop_map(move |(mut slots, ranks, plantings)| {
                    for (variable, (pair, forms)) in vars.iter().zip(&plantings) {
                        for (position, form) in pair.iter().zip(forms) {
                            slots[*position] = planted(&world, *variable, *form);
                        }
                    }
                    let ret = slots.pop().expect("the return is the last position");
                    let run: Vec<DispatchTokenElement> = key
                        .iter()
                        .zip(slots)
                        .flat_map(|(keyword, slot)| {
                            [
                                DispatchTokenElement::Keyword(*keyword),
                                DispatchTokenElement::Slot(slot),
                            ]
                        })
                        .collect();
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

/// Where a binder's `variables` are planted among its `positions`: two distinct positions each,
/// no position shared between two variables, and a [`planted`] form at each position. The second
/// position repeats the first's form half the time, so two bare or two wrapped occurrences stay
/// as frequent as a bare one beside a function's parameter.
fn arb_plantings(
    positions: usize,
    variables: usize,
) -> impl Strategy<Value = Vec<([usize; 2], [u8; 2])>> {
    debug_assert!(positions >= 2 * variables, "two positions per variable");
    (
        prop::sample::subsequence((0..positions).collect::<Vec<usize>>(), 2 * variables)
            .prop_shuffle(),
        prop::collection::vec((0..3u8, prop::option::of(0..3u8)), variables),
    )
        .prop_map(|(picked, forms)| {
            picked
                .chunks(2)
                .map(|pair| [pair[0], pair[1]])
                .zip(forms)
                .map(|(pair, (first, second))| (pair, [first, second.unwrap_or(first)]))
                .collect()
        })
}

/// A function type of one to three parameters, over a quantifier group of its own where
/// `vocabulary` allows or demands one. Every variable is minted under a variable-free bound.
///
/// A variable is planted at **two** positions — two parameters, or a parameter and the return —
/// for the reason [`arb_shape_over`] plants one at two, so the group has at most
/// `(arity + 1) / 2` variables. Each occurrence takes a [`planted`] form, as there.
fn arb_function(
    world: World,
    depth: u32,
    members: Rc<Vec<Handle>>,
    vocabulary: Vocabulary,
) -> BoxedStrategy<Handle> {
    let grounds = world.grounds().len();
    // The return is the last plantable position, so even a one-parameter function gives a variable
    // two places to occur.
    (1..=world.binders.len())
        .prop_flat_map(move |arity| {
            (
                Just(arity),
                prop::collection::vec(0..grounds, vocabulary.group_sizes(arity + 1)),
            )
        })
        .prop_flat_map(move |(arity, bounds)| {
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
            let position = arb_type_in(
                world.clone(),
                depth.saturating_sub(1),
                vars.clone(),
                members.clone(),
                vocabulary.nested(),
            );
            (
                prop::collection::vec(position, arity + 1),
                arb_plantings(arity + 1, vars.len()),
            )
                .prop_map(move |(mut positions, plantings)| {
                    for (variable, (pair, forms)) in vars.iter().zip(&plantings) {
                        for (position, form) in pair.iter().zip(forms) {
                            positions[*position] = planted(&world, *variable, *form);
                        }
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

/// A planted variable in one of four forms: itself (`form` 0), a list's element (1), a function's
/// parameter (2), which reaches the variable from above, or a record's field (3).
fn planted(world: &World, variable: Handle, form: u8) -> Handle {
    match form {
        0 => variable,
        1 => world.types.list(variable),
        2 => function_of(world, variable),
        _ => with_scratch(|scratch| world.types.record(scratch, &[(world.binders[0], variable)])),
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
pub fn arb_signature_type(world: World, depth: u32) -> BoxedStrategy<KType> {
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
pub fn arb_signature(world: World, depth: u32) -> BoxedStrategy<KType> {
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
                Vocabulary::CLOSED,
            );
            let slot = arb_type_in(
                world.clone(),
                depth - 1,
                none.clone(),
                members.clone(),
                Vocabulary::ANY,
            );
            let keyworded = arb_shape(
                world.clone(),
                depth.clamp(1, 2),
                members.clone(),
                Vocabulary::ANY,
            );
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
                            let kt = world
                                .declared(kt)
                                .as_type()
                                .expect("a closed draw is a type");
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
    arb_shape(world.clone(), depth, Rc::new(Vec::new()), Vocabulary::ANY)
        .prop_map(move |raw| world.declared(raw))
        .boxed()
}

/// Two shapes over one bucket key — what every law that ranks or admits one shape against another
/// needs, and which two independent draws share only by accident.
pub fn arb_shape_pair(
    world: World,
    depth: u32,
    vocabulary: Vocabulary,
) -> BoxedStrategy<(DeclaredType<Parametric>, DeclaredType<Parametric>)> {
    let shapes = world.clone();
    arb_key(world.clone(), 1..5)
        .prop_flat_map(move |key| {
            let shape = || {
                let none = Rc::new(Vec::new());
                arb_shape_over(shapes.clone(), key.clone(), depth, none, vocabulary)
            };
            (shape(), shape())
        })
        .prop_map(move |(a, b)| (world.declared(a), world.declared(b)))
        .boxed()
}

/// A candidate shape drawn under `candidate` and a shape over its key and ranking, binding no
/// group, whose slots are arguments for it: three times in four the candidate's own slots at
/// bindings within its group's bounds — each slot at a binding-pool type or, where `candidate` has
/// variables and the slot's coin says so, at the lexical variable over that type, so one slot
/// sits at the type another holds the variable over — each kept or met with a pool type;
/// otherwise an unrelated draw. A slot whose instance holds a binder takes the unrelated draw's
/// slot, and one whose meet is `Never` stays unmet. Two independent draws admit one another only
/// by accident.
pub fn arb_argument_pair(
    world: World,
    depth: u32,
    candidate: Vocabulary,
) -> BoxedStrategy<(DeclaredType<Parametric>, DeclaredType<Parametric>)> {
    let pool = world.argument_pool();
    let (shapes, pools) = (world.clone(), pool.len());
    let bindings = world.clone();
    arb_key(world.clone(), 1..5)
        .prop_flat_map(move |key| {
            let positions = key.len();
            let none = || Rc::new(Vec::new());
            (
                arb_shape_over(shapes.clone(), key.clone(), depth, none(), candidate),
                arb_shape_over(shapes.clone(), key, depth, none(), Vocabulary::CONCRETE),
                prop::bool::weighted(0.75),
                // One binding per name: more than any group binds, and zipped with the bounds.
                prop::collection::vec(arb_binding(&bindings), bindings.type_names.len()),
                prop::collection::vec(any::<bool>(), positions),
                prop::collection::vec(prop::option::of(0..pools), positions),
            )
        })
        .prop_map(move |(a, unrelated, own, picks, lexicals, narrowings)| {
            if !own {
                return (world.declared(a), world.declared(unrelated));
            }
            let types = &world.types;
            let (
                TypeNode::ExpressionShape { classes, .. },
                TypeNode::ExpressionShape { elements, ret, .. },
            ) = (types.node(a), types.node(unrelated))
            else {
                unreachable!("a drawn shape is one");
            };
            let b = with_scratch(|scratch| {
                let bind = |lexical: bool| -> Vec<Handle> {
                    substitute::quantifier_bounds(types, a)
                        .iter()
                        .zip(&picks)
                        .enumerate()
                        .map(|(index, (bound, pick))| {
                            world.bind_within(scratch, *bound, index, pick, lexical)
                        })
                        .collect()
                };
                let (plain, variables) = (bind(false), bind(candidate.variables));
                let mut own = shape_slots(a, types).zip(&narrowings).zip(&lexicals);
                let run: Vec<DispatchTokenElement> = elements
                    .iter()
                    .map(|element| match element {
                        DispatchTokenElement::Slot(unrelated) => {
                            let ((slot, narrowing), lexical) =
                                own.next().expect("one key, one arity");
                            let bindings = if *lexical { &variables } else { &plain };
                            let instance =
                                substitute::substitute_quantified(types, scratch, slot, bindings);
                            DispatchTokenElement::Slot(if holds_binder(types, scratch, instance) {
                                *unrelated
                            } else {
                                narrowing
                                    .map(|index| {
                                        let other = pool[index].raw();
                                        lattice::meet_through_variables(
                                            types, scratch, instance, other,
                                        )
                                    })
                                    .filter(|met| *met != Handle::NEVER)
                                    .unwrap_or(instance)
                            })
                        }
                        keyword => *keyword,
                    })
                    .collect();
                types.shape_group(scratch, &[], &[], &run, classes, ret).0
            });
            (world.declared(a), world.declared(b))
        })
        .boxed()
}

/// A monomorphic shape `b` and a shape `a` below it, over one key and one ranking: each of `b`'s
/// slots kept or widened, since a slot is contravariant, and its return kept or met with a pool
/// type.
pub fn arb_shape_below(
    world: World,
    depth: u32,
) -> BoxedStrategy<(DeclaredType<Parametric>, DeclaredType<Parametric>)> {
    let pool = world.widening_pool();
    let (shapes, pools) = (world.clone(), pool.len());
    arb_key(world.clone(), 1..5)
        .prop_flat_map(move |key| {
            let positions = key.len();
            let none = Rc::new(Vec::new());
            (
                arb_shape_over(shapes.clone(), key, depth, none, Vocabulary::CONCRETE),
                prop::collection::vec(prop::option::of(arb_widening(pools, 1)), positions),
                prop::option::of(0..pools),
            )
        })
        .prop_map(move |(b, widenings, narrowing)| {
            let TypeNode::ExpressionShape {
                elements,
                classes,
                ret,
                ..
            } = world.types.node(b)
            else {
                unreachable!("a drawn shape is one");
            };
            let a = with_scratch(|scratch| {
                let mut widenings = widenings.iter();
                let run: Vec<DispatchTokenElement> = elements
                    .iter()
                    .map(|element| match element {
                        DispatchTokenElement::Slot(slot) => {
                            DispatchTokenElement::Slot(match widenings.next() {
                                Some(Some(widening)) => {
                                    widen(&world, scratch, &pool, *slot, widening)
                                }
                                _ => *slot,
                            })
                        }
                        keyword => *keyword,
                    })
                    .collect();
                let ret = match narrowing {
                    Some(index) => meet(&world.types, scratch, world.concrete(ret), pool[index]),
                    None => world.concrete(ret),
                };
                world
                    .types
                    .shape_group(scratch, &[], &[], &run, classes, ret.raw())
                    .0
            });
            (world.declared(a), world.declared(b))
        })
        .boxed()
}

/// A generated function type, for the laws whose subject is a function and which a draw from the
/// whole vocabulary would leave mostly vacuous.
pub fn arb_function_type(world: World, depth: u32) -> BoxedStrategy<DeclaredType<Parametric>> {
    arb_function(world.clone(), depth, Rc::new(Vec::new()), Vocabulary::ANY)
        .prop_map(move |raw| world.declared(raw))
        .boxed()
}

/// A function scheme drawn under `scheme` and a function type it may be wanted at, drawn under
/// `wanted`: an unrelated function type, or — three times as often — the scheme's own instance at
/// bindings within its bounds, as it is or widened, under which an instance always lies. An
/// unrelated draw alone almost never meets its scheme.
///
/// `scheme` binds its group at the outermost function, so it is `Always` or `Outer`; the unrelated
/// type binds none. Where `wanted` has variables an own instance binds each variable to a lexical
/// variable over a ground within its bound — above a lower end of one member where that ground
/// spans two — and to the ground otherwise.
pub fn arb_wanted_instance(
    world: World,
    depth: u32,
    scheme: Vocabulary,
    wanted: Vocabulary,
) -> BoxedStrategy<(Scheme, Parametric)> {
    let unrelated = arb_function(
        world.clone(),
        depth,
        Rc::new(Vec::new()),
        Vocabulary {
            groups: Groups::Never,
            ..wanted
        },
    )
    .prop_map({
        let world = world.clone();
        move |raw| {
            world
                .declared(raw)
                .as_type()
                .expect("a function without a group is a type")
        }
    });
    prop_oneof![
        3 => arb_own_instance(world.clone(), depth, scheme, wanted),
        1 => (arb_scheme(world, depth, scheme), unrelated),
    ]
    .boxed()
}

/// [`arb_wanted_instance`]'s own instances alone: a scheme and its instance at bindings within its
/// bounds — half the time widened by a [`Widening`] that keeps it a function type, so the instance
/// lies strictly under the wanted type and the solve has an interval to choose from — so an
/// instance always lies under the wanted type.
pub fn arb_own_instance(
    world: World,
    depth: u32,
    scheme: Vocabulary,
    wanted: Vocabulary,
) -> BoxedStrategy<(Scheme, Parametric)> {
    let pool = world.widening_pool();
    // One binding per name: more than any group binds, and zipped with the bounds.
    let bindings = prop::collection::vec(arb_binding(&world), world.type_names.len());
    let widening = prop::option::of(arb_function_widening(pool.len(), 2));
    (arb_scheme(world.clone(), depth, scheme), bindings, widening)
        .prop_map(move |(scheme, picks, widening)| {
            with_scratch(|scratch| {
                let types = &world.types;
                let bindings: Vec<Handle> = quantifier_bounds(types, scheme)
                    .iter()
                    .zip(&picks)
                    .enumerate()
                    .map(|(index, (bound, pick))| {
                        world.bind_within(scratch, *bound, index, pick, wanted.variables)
                    })
                    .collect();
                let instance = instantiate_quantified(types, scratch, scheme, &bindings).raw();
                let wanted = match &widening {
                    Some(widening) => widen(&world, scratch, &pool, instance, widening),
                    None => instance,
                };
                // A function of full arity has no parameter to gain, so its step of width is a
                // union: an instance is wanted at a function type.
                let wanted = match types.node(wanted) {
                    TypeNode::KFunction { .. } => wanted,
                    _ => instance,
                };
                let wanted = world
                    .declared(wanted)
                    .as_type()
                    .expect("a function binding no group is a type");
                (scheme, wanted)
            })
        })
        .boxed()
}

/// A function scheme drawn under `vocabulary`, which binds its group at the outermost function.
fn arb_scheme(world: World, depth: u32, vocabulary: Vocabulary) -> BoxedStrategy<Scheme> {
    assert!(matches!(vocabulary.groups, Groups::Always | Groups::Outer));
    arb_function(world.clone(), depth, Rc::new(Vec::new()), vocabulary)
        .prop_map(move |raw| {
            world
                .declared(raw)
                .as_scheme()
                .expect("a function with a group is a scheme")
        })
        .boxed()
}

/// A tuple of argument types for a shape of `arity` positions, drawn from the ground alphabet plus a
/// couple of composites — what the admission laws feed a candidate.
pub fn arb_arguments(world: World, arity: usize) -> impl Strategy<Value = Vec<KType>> + use<> {
    let pool = world.argument_pool();
    prop::collection::vec(0..pool.len(), arity)
        .prop_map(move |picks| picks.into_iter().map(|index| pool[index]).collect())
}

/// One step up the order, read down it at a contravariant child ([`Way`]): to the top, beside a
/// pool type, a step of width, or the same step at one child.
#[derive(Clone, Debug)]
pub enum Widening {
    /// `Any` upward, `Never` downward.
    Top,
    /// The union with a pool type upward, the meet with one downward.
    UnionWith(usize),
    /// Upward, a record drops a field, a function gains a parameter, a signature application
    /// drops a pin, a meet of applications keeps one member and an application of a family steps
    /// to the family; downward, a record gains a field and a function drops a parameter. Where the
    /// node has no such step, [`Widening::UnionWith`].
    Width(usize),
    /// The same step at one child, read the other way at a contravariant one: a function's
    /// parameter or a monomorphic shape's slot.
    Inside(usize, Box<Widening>),
}

/// Which way a [`Widening`] is read: up the order, or down it under a contravariant child.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Way {
    Up,
    Down,
}

impl Way {
    /// The way a child of `variance` reads a step taken this way.
    fn through(self, variance: Variance) -> Way {
        match (self, variance) {
            (way, Variance::Co) => way,
            (Way::Up, Variance::Contra) => Way::Down,
            (Way::Down, Variance::Contra) => Way::Up,
        }
    }
}

/// A [`Widening`] over a pool of `pool` types, reaching at most `depth` children down. The top is
/// rare: a chain that reaches it early says nothing past it.
fn arb_widening(pool: usize, depth: u32) -> BoxedStrategy<Widening> {
    let leaf = prop_oneof![
        1 => Just(Widening::Top),
        5 => (0..pool).prop_map(Widening::UnionWith),
        2 => (0..pool).prop_map(Widening::Width),
    ];
    if depth == 0 {
        return leaf.boxed();
    }
    let inside = (0..4usize, arb_widening(pool, depth - 1))
        .prop_map(|(index, inner)| Widening::Inside(index, Box::new(inner)));
    prop_oneof![7 => leaf, 6 => inside].boxed()
}

/// A [`Widening`] that keeps a function type one: a step inside it, or one of width.
fn arb_function_widening(pool: usize, depth: u32) -> BoxedStrategy<Widening> {
    prop_oneof![
        3 => (0..4usize, arb_widening(pool, depth))
            .prop_map(|(index, inner)| Widening::Inside(index, Box::new(inner))),
        1 => (0..pool).prop_map(Widening::Width),
    ]
    .boxed()
}

/// `kt` widened by `widening` over `pool`: above `kt` whatever its node.
fn widen(
    world: &World,
    scratch: BumpAllocator<'_>,
    pool: &[KType],
    kt: Handle,
    widening: &Widening,
) -> Handle {
    step(world, scratch, pool, kt, widening, Way::Up)
}

/// `kt` stepped by `widening` the way `way` reads it: above `kt` upward, below it downward.
fn step(
    world: &World,
    scratch: BumpAllocator<'_>,
    pool: &[KType],
    kt: Handle,
    widening: &Widening,
    way: Way,
) -> Handle {
    let types = &world.types;
    let beside = |index: usize| {
        let other = pool[index % pool.len()].raw();
        match way {
            Way::Up => types.union_of(scratch, &[kt, other]),
            Way::Down => lattice::meet_through_variables(types, scratch, kt, other),
        }
    };
    let (index, inner) = match widening {
        Widening::Top => {
            return match way {
                Way::Up => Handle::ANY,
                Way::Down => Handle::NEVER,
            };
        }
        Widening::UnionWith(index) => return beside(*index),
        Widening::Width(index) => {
            return step_width(world, scratch, pool, kt, *index, way)
                .unwrap_or_else(|| beside(*index));
        }
        Widening::Inside(index, inner) => (*index, inner),
    };
    let at = |child: Handle, variance: Variance| {
        step(world, scratch, pool, child, inner, way.through(variance))
    };
    match types.node(kt) {
        TypeNode::List { element } => types.list(at(element, Variance::Co)),
        TypeNode::Dict { key, value } if index % 2 == 0 => types.dict(key, at(value, Variance::Co)),
        TypeNode::Dict { key, value } => types.dict(at(key, Variance::Co), value),
        TypeNode::Record { fields } if !fields.raw().is_empty() => {
            let mut fields = fields.raw().to_vec();
            let chosen = index % fields.len();
            fields[chosen].1 = at(fields[chosen].1, Variance::Co);
            types.record(scratch, &fields)
        }
        TypeNode::Union { members } => {
            let mut members = members.to_vec();
            let chosen = index % members.len();
            members[chosen] = at(members[chosen], Variance::Co);
            types.union_of(scratch, &members)
        }
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } if !arguments.raw().is_empty() => {
            let mut arguments = arguments.raw().to_vec();
            let chosen = index % arguments.len();
            arguments[chosen].1 = at(arguments[chosen].1, Variance::Co);
            types.constructor_apply(scratch, world.concrete(constructor), &arguments)
        }
        // A parameter is contravariant and the return, the last position, covariant.
        TypeNode::KFunction {
            quantifiers: &[],
            params,
            ret,
            ..
        } => {
            let mut params = params.raw().to_vec();
            let chosen = index % (params.len() + 1);
            if chosen == params.len() {
                return types.function_type(scratch, &params, at(ret, Variance::Co));
            }
            params[chosen].1 = at(params[chosen].1, Variance::Contra);
            types.function_type(scratch, &params, ret)
        }
        // A monomorphic shape's slot is contravariant and its return, the last position, covariant.
        TypeNode::ExpressionShape {
            quantifiers: &[],
            elements,
            classes,
            ret,
            ..
        } => {
            let slots = elements
                .iter()
                .filter(|element| matches!(element, DispatchTokenElement::Slot(_)))
                .count();
            let chosen = index % (slots + 1);
            if chosen == slots {
                let ret = at(ret, Variance::Co);
                return types
                    .shape_group(scratch, &[], &[], &elements[..], classes, ret)
                    .0;
            }
            let mut seen = 0;
            let run: Vec<DispatchTokenElement> = elements
                .iter()
                .map(|element| match element {
                    DispatchTokenElement::Slot(slot) => {
                        let slot = if seen == chosen {
                            at(*slot, Variance::Contra)
                        } else {
                            *slot
                        };
                        seen += 1;
                        DispatchTokenElement::Slot(slot)
                    }
                    keyword => *keyword,
                })
                .collect();
            types.shape_group(scratch, &[], &[], &run, classes, ret).0
        }
        _ => beside(index),
    }
}

/// [`Widening::Width`] at `kt`, or `None` where its node has no step of width the way `way` reads
/// it. A name a record or function gains is the first of the binder alphabet, from `index`, it
/// lacks; a function drops a parameter only when it keeps one.
fn step_width(
    world: &World,
    scratch: BumpAllocator<'_>,
    pool: &[KType],
    kt: Handle,
    index: usize,
    way: Way,
) -> Option<Handle> {
    let types = &world.types;
    let other = pool[index % pool.len()].raw();
    let gain = |held: &[(BinderSymbol, Handle)]| -> Option<Vec<(BinderSymbol, Handle)>> {
        let binders = &world.binders;
        let name = (0..binders.len())
            .map(|offset| binders[(index + offset) % binders.len()])
            .find(|name| held.iter().all(|(taken, _)| taken != name))?;
        let mut fields = held.to_vec();
        fields.push((name, other));
        Some(fields)
    };
    let drop = |held: &[(BinderSymbol, Handle)]| -> Vec<(BinderSymbol, Handle)> {
        let mut fields = held.to_vec();
        fields.remove(index % fields.len());
        fields
    };
    Some(match (types.node(kt), way) {
        (TypeNode::Record { fields }, Way::Up) if !fields.raw().is_empty() => {
            types.record(scratch, &drop(fields.raw()))
        }
        (TypeNode::Record { fields }, Way::Down) => types.record(scratch, &gain(fields.raw())?),
        (
            TypeNode::KFunction {
                quantifiers: &[],
                params,
                ret,
                ..
            },
            Way::Up,
        ) => types.function_type(scratch, &gain(params.raw())?, ret),
        (
            TypeNode::KFunction {
                quantifiers: &[],
                params,
                ret,
                ..
            },
            Way::Down,
        ) if params.raw().len() >= 2 => types.function_type(scratch, &drop(params.raw()), ret),
        (TypeNode::SignatureApply { signature, pins }, Way::Up) => {
            types.signature_apply(scratch, signature, &drop(pins.raw()))
        }
        (TypeNode::SignatureMeet { members }, Way::Up) => members[index % members.len()],
        (TypeNode::ConstructorApply { constructor, .. }, Way::Up)
            if matches!(
                types.node(constructor),
                TypeNode::SetMember { .. } | TypeNode::Sibling(_)
            ) =>
        {
            constructor
        }
        _ => return None,
    })
}

/// `a ≤ b` by construction: `b` is `a` restated — a union of one member, or its node re-interned —
/// or `a` widened.
pub fn arb_ordered_pair(world: World, depth: u32) -> BoxedStrategy<(KType, KType)> {
    let pool = world.widening_pool();
    (
        arb_concrete(world.clone(), depth),
        prop::option::weighted(0.8, arb_widening(pool.len(), 2)),
        any::<bool>(),
    )
        .prop_map(move |(a, widening, union)| {
            let b = with_scratch(|scratch| match widening {
                Some(widening) => widen(&world, scratch, &pool, a.raw(), &widening),
                None if union => world.types.union_of(scratch, &[a.raw()]),
                None => world.types.intern(scratch, world.types.node(a.raw())),
            });
            (a, world.concrete(b))
        })
        .boxed()
}

/// `a ≤ b ≤ c` by construction, each step a widening.
pub fn arb_chain(world: World, depth: u32) -> BoxedStrategy<(KType, KType, KType)> {
    let pool = world.widening_pool();
    (
        arb_concrete(world.clone(), depth),
        arb_widening(pool.len(), 2),
        arb_widening(pool.len(), 2),
    )
        .prop_map(move |(a, first, second)| {
            with_scratch(|scratch| {
                let b = widen(&world, scratch, &pool, a.raw(), &first);
                let c = widen(&world, scratch, &pool, b, &second);
                (a, world.concrete(b), world.concrete(c))
            })
        })
        .boxed()
}

/// A *fits* chain over an order chain `a ≤ b ≤ c`: the order chain itself; one whose bottom is a
/// lexical variable bounded by `a`, which fits `b` through its bound; or one whose middle is a
/// lexical variable between `a` and `b`, which `a` fits through its lower end and which fits `c`
/// through its bound. A type holding an opaque carrier bounds no variable, and a pair converged or
/// resting on `Never` encloses none with a lower end, so such a chain is drawn as the order's.
pub fn arb_fits_chain(
    world: World,
    depth: u32,
) -> BoxedStrategy<(
    DeclaredType<Parametric>,
    DeclaredType<Parametric>,
    DeclaredType<Parametric>,
)> {
    (arb_chain(world.clone(), depth), 0..3u8)
        .prop_map(move |((a, b, c), route)| {
            let types = &world.types;
            let name = world.type_names[0];
            let bounds = |kt: KType| !types.holds_carrier(kt);
            let (a, b) = match route {
                1 if bounds(a) => (types.lexical(0, name, a).raw(), b.raw()),
                2 if a != KType::NEVER && a != b && bounds(a) && bounds(b) => (
                    a.raw(),
                    with_scratch(|scratch| types.lexical_between(scratch, 0, name, a, b)).raw(),
                ),
                _ => (a.raw(), b.raw()),
            };
            (
                world.declared(a),
                world.declared(b),
                world.declared(c.raw()),
            )
        })
        .boxed()
}

/// A *fits* chain of signatures through a bounded head parameter, which [`arb_fits_chain`] never
/// draws: an opaque view's signature, its slot at a carrier recorded under one ground; a signature
/// over a head parameter bounded by that ground, its slot at the parameter; and a signature over a
/// parameter bounded by another ground, its slot at that parameter, or one whose slot is that
/// ground, or one over a parameter bounded by that ground as the slots hold it, its slot the bare
/// parameter. Each slot holds its carrier, parameter or ground at one [`planted`] position — bare,
/// a list's element, a function's parameter or a record's field — so the last kind checks a head's
/// bound at both variances. Not every step fits: a bound decides which modules fit and reveals
/// nothing, so a bounded parameter fits no ground.
pub fn arb_bounded_head_chain(world: World) -> BoxedStrategy<(KType, KType, KType)> {
    let grounds = world.grounds().len();
    (0..grounds, 0..grounds, 0..3u8, 0..4u8)
        .prop_map(move |(bound, target, last, form)| {
            let types = &world.types;
            let (name, slot) = (world.type_names[0], world.values[0]);
            let (bound, target) = (world.grounds()[bound], world.grounds()[target]);
            let at = |kt: Handle| planted(&world, kt, form);
            with_scratch(|scratch| {
                let signature = |parameter: Option<Parametric>, read: Handle| {
                    let mut draft = SchemaDraft::new(scratch);
                    if let Some(parameter) = parameter {
                        draft.insert_parameter(name, parameter);
                    }
                    draft.origin = SigOrigin::Declared;
                    draft.insert_value_slot(slot, world.declared(read));
                    draft
                };
                let carrier = types.carrier(name, bound, OPAQUE_KEY);
                let mut view = signature(None, at(carrier.raw()));
                view.insert_manifest(name, carrier);
                view.origin = SigOrigin::Module;
                let view = types.signature(scratch, view);
                let over = |bound| {
                    let parameter = types.head_parameter(name, bound);
                    types.signature(scratch, signature(Some(parameter), at(parameter.raw())))
                };
                let last = match last {
                    0 => over(target),
                    1 => types.signature(scratch, signature(None, at(target.raw()))),
                    _ => {
                        let parameter =
                            types.head_parameter(name, world.concrete(at(target.raw())));
                        types.signature(scratch, signature(Some(parameter), parameter.raw()))
                    }
                };
                (view, over(bound), last)
            })
        })
        .boxed()
}

/// A chain `a ≤ b ≤ c` through an instance, which [`arb_any`]'s draws almost never line up: `a` is
/// a binder over one variable at two positions, `b` its instance at the least instance `c`'s two
/// positions give that variable, and `c` puts two ground types at those positions.
///
/// The binder is a function, or a shape with both slots in one class. A position is the variable
/// itself — two ground types join into it — or `FN :{x :_} -> Null` over it, where they meet; or
/// one of each, where the bare position's ground pours in from below, the wrapped one holds its
/// join with the other ground as a cap, and the instance is the bare one's ground. An optional
/// filler position holds one generated type in all three.
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
    let wrapped = prop::sample::select(vec![
        [false, false],
        [true, true],
        [false, true],
        [true, false],
    ]);
    (any::<bool>(), 0..pool.len(), 0..pool.len(), wrapped, filler)
        .prop_map(move |(shape, g1, g2, wrapped, filler)| {
            let (g1, g2) = (pool[g1], pool[g2]);
            let position = |kt, wrap: bool| if wrap { function_of(&world, kt) } else { kt };
            let variable = world.types.quantified(0, KType::ANY).raw();
            // The two grounds and the least instance the binder has at them.
            let ([g1, g2], instance) = with_scratch(|scratch| match wrapped {
                [false, false] => ([g1, g2], join(&world.types, scratch, g1, g2)),
                [true, true] => ([g1, g2], meet(&world.types, scratch, g1, g2)),
                [false, true] => ([g1, join(&world.types, scratch, g1, g2)], g1),
                [true, false] => ([join(&world.types, scratch, g1, g2), g2], g2),
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
                position(variable, wrapped[0]),
                position(variable, wrapped[1]),
            );
            let b = build(
                &[],
                position(instance, wrapped[0]),
                position(instance, wrapped[1]),
            );
            let c = build(&[], position(g1, wrapped[0]), position(g2, wrapped[1]));
            (world.declared(a), world.declared(b), world.declared(c))
        })
        .boxed()
}
