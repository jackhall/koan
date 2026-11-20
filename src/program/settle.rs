//! What a call seeks.
//!
//! A call hands the drain a [`Seek`], whatever its frame's placement: the depot slot the callee was
//! born under and, where the callee's body keys a settlement — its body's record carries the key
//! list the fold laid, so the asker reads no shape table — a request. A call at a site whose
//! request its frame's layer or the base carries hands the drain that request, laid with the
//! layer, and digests nothing. Any other computes the key of its segment under the content at
//! each keyed name and the image of the types that content carries. A parameter's content is its argument; a `FOR ALL`
//! name's is the call's solution at it, digested as the type value the frame binds: carried by a
//! keyworded call, held by an instance, and solved here, on the asker's region, for a call by
//! name. Where the callee's body can birth a callable, the request is laid as the frame's
//! [level] — by the frame's first step, from the request the drain hands it back — which a
//! callable born under the settlement records in its [stamp](crate::scope::Stamp), so a call of
//! it replays the chain where it was evicted. A call
//! past a header or a barrier of a body keyed on a `FOR ALL` name seeks no settlement, since its
//! frame solves only once the arguments are retyped or have crossed, unless it calls an instance,
//! whose solution holds wherever it is called from; one keyed on a module seeks none at all, since
//! its frame binds the module as it crossed, and decides in its activation.
//!
//! A keyed name holds **load content** where the load could have declared it: a type whose handle
//! rests in the root, a type value naming one, or a module whose content digest names a closed tree
//! the callee's chain holds: under the root, the load's index alone, checked here; under a
//! settlement or a composition, resolved by the drain's miss through that chain — and, under a
//! composition, through the composition a module of other composed code was born of, which the
//! settlement then pins. Anything else is content the run produced, and a call keying any seeks
//! nothing, or settles nothing. A miss runs [`settled`]: the language's walk rooted at the callee's
//! body, over a layer on the settlement's parent and its base's shapes, the binding's types rested
//! only once its trees resolve. Under the root they rest in the agent tier, as the walk's types do
//! once it ends; under a composition they rest in the walk's own chain, and the settlement keeps a
//! term of each type its records name instead. A callee born of code composed at run time keys its
//! settlements under its composition: its frame, born under the root where that composition is
//! gone, bounces with a copy of the syntax its body record's root holds. A composition's miss runs
//! [`composed`]: its code's shapes built alone over that copy and walked as the load walks the
//! program's.
//!
//! A miss and the load lay a layer by one step, [`layer_of`]: the walks under the asked binding
//! — a [resumption](super::resume) over a witnessed layer whose questions it keeps, witnessed
//! afresh, or closed — then [`requests_of`] the body's call sites that pass on its slots, each the
//! key and ask of the binding the site passes under the layer's. The load's pass, [`traced`], lays
//! the requests of the sites passing load content alone on the base, then find-or-lays a layer for
//! each binding the trace names, beneath the layer its callee is born under — over a witnessed
//! layer it laid for the body beneath that same layer, where the binding keeps its questions —
//! and refuses the load where one of them refused. A site passing on a slot of an open owner (one
//! past the cap, say) is laid under that owner's first layer alone. A miss under the root or a
//! load layer reads over a witnessed layer the load laid for its body beneath it, where its
//! binding keeps the questions, and witnesses nothing itself. See
//! [build/README.md](../build/README.md).

use smallvec::SmallVec;

use crate::build::{LayerStore, LoadStore, fold, key_of_ask};
use crate::knot::{Function, KValue, Knotted};
use crate::memory::{Bump, BumpBackedSet, BumpVec, ProgramBrand, Writer, bump_set, resident};
use crate::parse::KExpression;
use crate::scheduler::{Ask, Asked, Seek, Sought};
use crate::scope::{
    Address, Binding, Bound, BuildLayer, CallTrace, Composition, Content, EventSite, Facts, Fixed,
    Flattened, Keyed, Level, Levels, LoadFacts, LoadLayer, Named, Segment, ShapeError, ShapeKind,
    Shapes, Site, SiteRequest, Slot, SlotMap, Spellings, Tree, Unknown, Untraced, Written,
    quote_paths, relaid_error,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{DeclaredType, KType, Parametric, Term, TypeDigest, TypeNode};
use crate::values::{
    BindingImage, Chain, DepotRef, KeyedTypes, Knotted as _, LayerKey, Value, retype_target,
};

use super::body::admitted;
use super::bundle::KBirth;
use super::record::{CallKind, Language, Program};
use super::resume::{self, Resumed, Resuming};

/// The function a call of `callee` runs, behind every barrier: `None` for a callee that is no
/// function.
pub fn called_function<'graph, 'cell>(
    callee: KValue<'graph, 'cell>,
) -> Option<&'cell Function<'graph, 'cell, Knotted<'graph, 'cell>>> {
    callee.as_callable()?.behind_barriers().function()
}

/// The parameter slots the call `birth`'s callee keys a settlement on, beside their names: `None`
/// for a birth that is no call, a callee that is no function, and a body that keys none.
pub fn callee_keys<'here>(birth: &KBirth<'_, 'here>) -> Option<&'here [(Slot, BinderSymbol)]> {
    let KBirth::Call { callee, .. } = birth else {
        return None;
    };
    called_function(*callee)?.body().keys
}

/// What the call `birth` seeks: the depot slot its callee was born under — the root for a birth
/// that is no call or a callee that is no function — and the settlement under it its body keys,
/// `keys`, the callee's as the caller read them ([`callee_keys`]). A call at a site whose
/// `request` a layer carries hands the drain that request, with an empty image, since every type
/// it names rests; any other computes its key from the values, if the asker can read its content
/// at every keyed name. `types` is the asker's evaluation chain, which reads the arguments;
/// `region` its region's, where a call by name's solution is solved. Where [`handed_back`], the
/// request is handed back to the frame's first step, which lays it as its level in its own region
/// ([`level`]).
pub fn seek<'graph>(
    birth: &mut KBirth<'graph, '_>,
    keys: Option<&[(Slot, BinderSymbol)]>,
    request: Option<&SiteRequest<'_>>,
    types: &Chain<'_, '_, '_, '_>,
    region: &Chain<'_, '_, '_, '_>,
) -> Seek<'graph> {
    let (function, program) = match birth {
        KBirth::Call {
            callee, program, ..
        } => (called_function(*callee), Some(*program)),
        _ => (None, None),
    };
    let parent = function.map_or(DepotRef::ROOT, |function| function.closure().build());
    let settlement = match request {
        Some(request) => {
            debug_assert_eq!(
                function.map(|function| function.body().segment),
                Some(request.body),
                "a site's request names its callee's body"
            );
            let sought = request.sought();
            #[cfg(test)]
            {
                if let Some(requesting) = REQUESTING.with(std::cell::Cell::get) {
                    requesting(birth, &sought);
                }
                if CROSS_CHECK.with(std::cell::Cell::get) {
                    let computed = keys.and_then(|keys| settlement(birth, keys, types, region));
                    assert_eq!(
                        computed
                            .as_ref()
                            .map(|computed| (computed.key, &computed.ask)),
                        Some((sought.key, &sought.ask)),
                        "a request names the key and ask its values give"
                    );
                }
            }
            Some((BindingImage::default(), sought))
        }
        None => {
            let settlement = keys.and_then(|keys| settlement(birth, keys, types, region));
            #[cfg(test)]
            if let (Some(sought), Some(seeking)) = (&settlement, SEEKING.with(std::cell::Cell::get))
            {
                seeking(birth, sought);
            }
            #[cfg(test)]
            if let (Some(_), None, Some(unsought)) =
                (keys, &settlement, UNSOUGHT.with(std::cell::Cell::get))
            {
                unsought(birth);
            }
            settlement.map(|sought| {
                let tiers = program.expect("a keyed birth is a call").tiers();
                (BindingImage::of(sought.keyed.clone(), tiers, types), sought)
            })
        }
    };
    let hands_back = settlement.is_some() && handed_back(birth);
    Seek {
        parent,
        settlement,
        hands_back,
    }
}

/// Whether the request of the call `birth` is handed back to its frame's first step, which lays it
/// as its level: where its callee's body can birth a callable and the birth holds no level yet.
pub fn handed_back(birth: &KBirth<'_, '_>) -> bool {
    match birth {
        KBirth::Call {
            callee,
            level: None,
            ..
        } => called_function(*callee).is_some_and(|function| function.body().births),
        _ => false,
    }
}

/// What a test hands each keyed seek's birth and request to.
#[cfg(test)]
pub(crate) type Seeking = fn(&KBirth<'_, '_>, &Sought);

#[cfg(test)]
thread_local! {
    /// How many levels were laid.
    pub(crate) static LEVELS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// What each keyed seek's birth and request are handed to, while a test sets it.
    pub(crate) static SEEKING: std::cell::Cell<Option<Seeking>> =
        const { std::cell::Cell::new(None) };
    /// What each call into a keyed callee that seeks no settlement is handed, while a test sets
    /// it: a call that reads no request and cannot read its content at every keyed name.
    pub(crate) static UNSOUGHT: std::cell::Cell<Option<fn(&KBirth<'_, '_>)>> =
        const { std::cell::Cell::new(None) };
    /// What each seek read off a request is handed: its birth and request.
    pub(crate) static REQUESTING: std::cell::Cell<Option<Seeking>> =
        const { std::cell::Cell::new(None) };
    /// Whether a seek read off a request also computes its key from the values and asserts the two
    /// agree: set by one law alone.
    pub(crate) static CROSS_CHECK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The level of `sought`, the request of the call `birth`, its chain resting on `base`, laid through
/// `writer`: the one function that lays a level, which a frame's first step calls from the request
/// the drain hands it back, in its own region. It pins each keyed module of composed code other than the callee's own,
/// whose root rebuilds that code's composition on a replay; `types` reads those arguments.
pub fn level<'graph, 'here>(
    writer: Writer<'here, '_>,
    birth: &KBirth<'graph, 'here>,
    sought: &Sought,
    types: &Chain<'_, '_, '_, '_>,
    base: DepotRef,
) -> &'here Level<'here, Knotted<'graph, 'here>> {
    #[cfg(test)]
    LEVELS.with(|levels| levels.set(levels.get() + 1));
    let KBirth::Call {
        callee, arguments, ..
    } = birth
    else {
        unreachable!("a level is laid for a call");
    };
    let body = called_function(*callee)
        .expect("a call seeking a settlement calls a function")
        .body();
    let keys = body
        .keys
        .expect("a call seeking a settlement keys its body");
    let own = body.root.map(|root| root.digest);
    let scratch = Bump::new();
    let mut modules: SmallVec<[KValue<'graph, 'here>; 1]> = SmallVec::new();
    for asked in &sought.ask.bound {
        let Asked::Module {
            slot,
            composition: Some(composition),
            ..
        } = *asked
        else {
            continue;
        };
        if Some(composition) == own {
            continue;
        }
        let (_, name) = keys
            .iter()
            .find(|(keyed, _)| *keyed == slot)
            .expect("a keyed module names a key");
        let argument = arguments
            .field(name.symbol(), types, &scratch)
            .expect("a keyed module's argument was read when its request was");
        modules.push(argument.value());
    }
    Level::laid(writer, sought, &modules, base)
}

/// The request of the settlement the call `birth` seeks under its callee's depot slot, its body
/// keyed on `keyed`: `None` for a birth that is no call, a callee that is no function, and a call
/// whose content at a keyed name the asker cannot read or is not load content — a module passed
/// past a header or a barrier among them, which the frame binds only once retyped or crossed.
fn settlement<'graph, 'here>(
    birth: &KBirth<'graph, 'here>,
    keyed: &[(Slot, BinderSymbol)],
    types: &Chain<'_, '_, '_, '_>,
    region: &Chain<'_, '_, '_, '_>,
) -> Option<Sought> {
    let KBirth::Call {
        program,
        callee,
        arguments,
        kind,
        contributed,
        ..
    } = birth
    else {
        return None;
    };
    let (callee, arguments) = (*callee, *arguments);
    let function = called_function(callee)?;
    let body = function.body();
    let under_root = function.closure().build() == DepotRef::ROOT;
    let scratch = Bump::new();
    // Past a header or a barrier the frame solves its group and binds its arguments only once it
    // has retyped or crossed them, so neither is read off the caller's.
    let past = (callee.as_callable())
        .is_none_or(|node| node.coerced().is_some() || callee.ktype() != node.declared());
    // The call's solution of its group, solved at most once and only for a keyed `FOR ALL` name:
    // `None` past a header or a barrier.
    let solve = || -> Option<BumpVec<'_, KType>> {
        let mut solved = BumpVec::new_in(&scratch);
        if let Some(instance) = function.instance() {
            solved.extend_from_slice(instance);
            return Some(solved);
        }
        let node = callee.as_callable()?;
        if past {
            return None;
        }
        match kind {
            CallKind::Keyworded => {
                solved.resize(function.quantifier_map().iter().count(), KType::NEVER);
                for (name, index) in function.quantifier_map().iter() {
                    let field = arguments.field(name.symbol(), types, &scratch)?;
                    solved[index] = field.value().as_type()?.handle();
                }
                Some(solved)
            }
            CallKind::ByName => {
                let view = node.declared().view(region);
                admitted(region, &scratch, view, arguments, contributed).ok()?
            }
        }
    };
    // The declared type of the parameter `name`, where it is closed.
    let declared = |name: crate::symbols::Symbol| {
        let declared = callee.as_callable()?.declared().view(types);
        let TypeNode::KFunction { params, .. } = declared.body else {
            return None;
        };
        types.concrete(params.get(name)?)
    };
    let mut solution = None;
    let mut binding = SmallVec::new();
    let mut bound = SmallVec::new();
    for &(slot, name) in keyed {
        if let BinderSymbol::Type(name) = name
            && let Some(index) = function.quantifier_index(name)
        {
            if solution.is_none() {
                solution = Some(solve()?);
            }
            let solved = *solution.as_ref()?.get(index)?;
            if !region.in_root(solved) {
                return None;
            }
            let kind = KType::of_kind(solved.kind_of(region));
            binding.push(KeyedTypes {
                carried: kind.into(),
                named: Some(solved),
            });
            bound.push(Asked::Type {
                slot,
                named: solved,
            });
            continue;
        }
        let argument = arguments.field(name.symbol(), types, &scratch)?.value();
        let asked = match (argument.as_type(), argument.as_module()) {
            (Some(named), _) if region.in_root(named.handle()) => Asked::Type {
                slot,
                named: named.handle(),
            },
            // A module is load content where its bare member is: the type it is seen at retypes
            // the tree the load computed for it. Its tree resolves through the callee's chain —
            // the load alone under the root, and where the drain settles under a settlement.
            // Past a header or a barrier the frame binds this module retyped or re-viewed.
            (_, Some(_)) => {
                if past {
                    return None;
                }
                let member = argument.knot_member()?;
                let content = Value::Knotted(member).digest(types, &scratch);
                if under_root {
                    program.facts().tree_of(content)?;
                }
                let DeclaredType::Type(own) = member.ktype() else {
                    return None;
                };
                let declared = declared(name.symbol())?;
                let (carried, retyped) =
                    module_asked(types, &scratch, argument.ktype(), own, declared)?;
                // The type the frame binds the module at rests beside what it carries.
                binding.push(KeyedTypes {
                    carried: carried.into(),
                    named: None,
                });
                Asked::Module {
                    slot,
                    carried,
                    content,
                    retyped,
                    composition: (member.module())
                        .and_then(|module| module.body()?.root)
                        .map(|root| root.digest),
                }
            }
            _ => return None,
        };
        binding.push(KeyedTypes::of(&argument));
        bound.push(asked);
    }
    let ask = Ask {
        body: body.segment,
        bound,
    };
    let key = key_of_ask(body.base, &ask, region);
    Some(Sought {
        key,
        ask,
        keyed: binding,
    })
}

/// The type a frame binds a module argument of type `argument` at, retyped to `declared`, its
/// parameter's closed declared type, beside whether that differs from `own`, the bare module's
/// type: what an [`Asked::Module`] carries. The run reads a value's types, the load a static
/// type's. `None` where the frame's target is a scheme.
pub(crate) fn module_asked(
    types: &Chain<'_, '_, '_, '_>,
    scratch: &Bump,
    argument: DeclaredType<KType>,
    own: KType,
    declared: KType,
) -> Option<(KType, bool)> {
    match retype_target(types, scratch, argument, declared) {
        DeclaredType::Type(carried) => Some((carried, carried != own)),
        DeclaredType::Scheme(_) => None,
    }
}

/// The request of each covered call site under a binding, by shape and site, laid through `lay`:
/// of `owner`'s sites passing a slot of `owner`, under `ask` (its keys as `ask` binds them); or,
/// with no owner, of every site passing load content alone (the base's). A `Passed` key takes the
/// owner's `Asked` at that slot, re-slotted; a `Content` key gives an `Asked` from the content, a
/// module's carried type computed by [`module_asked`] over `types` from the type it carries in
/// (`at`). A site whose module retypes to a scheme, or whose owner's ask binds nothing at a slot it
/// passes on, carries none. Each type a request names is handed to `named`.
///
/// Re-slotting the owner's `Asked` is sound because the walk records a `Passed` key only where the
/// owner's parameter has the callee's own closed declared type, so retyping the module the owner
/// binds to the callee's declared type changes nothing, and for a type-channel slot only where the
/// point is exactly the owner's `FOR ALL` name or `:Type` parameter, which binds a type by its
/// handle. A debug assertion holds the first for a value parameter.
pub(crate) fn requests_of<'a>(
    lay: impl crate::memory::Lays<'a>,
    scratch: &Bump,
    load: &LoadFacts<'_>,
    owner: Option<(Segment, &Ask)>,
    types: &Chain<'_, '_, '_, '_>,
    named: &mut dyn FnMut(DeclaredType<Parametric>),
) -> Keyed<'a, ((Segment, Site), SiteRequest<'a>)> {
    let shapes = load.shapes();
    let base = shapes.root().code_digest();
    // The ask of the binding `map` passes, where every key's binding is known.
    let asked = |map: &SlotMap<'_>| -> Option<Ask> {
        let mut bound = SmallVec::new();
        for &(slot, fixed) in map.passed {
            let asked = match fixed {
                Fixed::Passed(from) => {
                    let (_, ask) = owner?;
                    let asked = *ask.bound.iter().find(|asked| asked.slot() == from)?;
                    #[cfg(debug_assertions)]
                    passed_at_its_own_type(
                        load,
                        types,
                        scratch,
                        (ask.body, from),
                        (map.callee, slot),
                        asked,
                    );
                    asked.at(slot)
                }
                Fixed::Content(Content::Type(named)) => Asked::Type { slot, named },
                Fixed::Content(Content::Module {
                    content,
                    at,
                    own,
                    declared,
                    ..
                }) => {
                    let (carried, retyped) =
                        module_asked(types, scratch, at.into(), own, declared)?;
                    Asked::Module {
                        slot,
                        carried,
                        content,
                        retyped,
                        composition: None,
                    }
                }
            };
            bound.push(asked);
        }
        Some(Ask {
            body: map.callee,
            bound,
        })
    };
    let mut staged = BumpVec::new_in(scratch);
    let mut request = |map: &SlotMap<'_>| {
        let Some(ask) = asked(map) else {
            return;
        };
        let keyed = (ask.bound.iter())
            .map(|asked| match *asked {
                Asked::Type { named, .. } => KeyedTypes {
                    carried: KType::of_kind(named.kind_of(types)).into(),
                    named: Some(named),
                },
                Asked::Module { carried, .. } => KeyedTypes {
                    carried: carried.into(),
                    named: None,
                },
            })
            .collect();
        let sought = Sought {
            key: key_of_ask(base, &ask, types),
            ask,
            keyed,
        };
        let request = SiteRequest::laid(lay, &sought);
        request.each_type(named);
        staged.push(((map.at, map.site), request));
    };
    match owner {
        Some((owner, _)) => (load.slot_maps(owner).iter())
            .filter(|map| map.passes_on())
            .for_each(&mut request),
        None => (shapes.bodies().iter())
            .flat_map(|shape| load.slot_maps(shape.segment()))
            .filter(|map| !map.passes_on())
            .for_each(&mut request),
    }
    Keyed::sorted(lay, &mut staged)
}

/// Asserts what re-slotting `asked`, the owner's binding at `from`, to the callee's key `to`
/// assumes for a module: the owner's parameter and the callee's share one closed declared type,
/// and retyping the bound module to it changes nothing.
#[cfg(debug_assertions)]
fn passed_at_its_own_type(
    load: &LoadFacts<'_>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: &Bump,
    from: (Segment, Slot),
    to: (Segment, Slot),
    asked: Asked,
) {
    let Asked::Module { carried, .. } = asked else {
        return;
    };
    // The closed declared type of the parameter at `slot` of the callable body `body`.
    let declared_at = |(body, slot): (Segment, Slot)| {
        let shape = load.shapes().shape_at(body);
        let crate::scope::Static::Closed(callable) = load.callable_type(types, shape) else {
            return None;
        };
        let TypeNode::KFunction { params, .. } = callable.ktype.view(types).body else {
            return None;
        };
        types.concrete(params.get(shape.slot_name(slot).symbol())?)
    };
    let declared = declared_at(to);
    assert!(
        declared.is_some(),
        "a module passed on at a closed declared type"
    );
    assert_eq!(
        declared_at(from),
        declared,
        "passed on at the callee's own declared type"
    );
    assert_eq!(
        retype_target(types, scratch, carried.into(), declared.unwrap_or(carried)),
        DeclaredType::Type(carried),
        "retyping the bound module to the callee's declared type changes nothing",
    );
}

/// The layer of `ask`'s body under its binding over `facts`, the layers below, and its call sites'
/// requests: the binding staged (a module's tree by `tree_of`), then `L`'s walks rooted at the
/// body, laid through `lay` and interned through `types` — over the first of `candidates`, the
/// witnessed layers laid for the body over the same layers, whose questions the binding keeps
/// most, else witnessed afresh where `witnesses` and the binding binds a type name, else closed
/// ([`resume`](super::resume)) — then [`requests_of`] the body under `ask` where `requests`, on
/// the binding's layer. Every type the layers laid, the refusal and the requests name is handed
/// to `named`; `resting` runs once the binding resolves, before a question is asked of it. `None` where
/// `tree_of` finds no tree for a module. What the load runs for each layer it lays and a miss
/// runs for its settlement.
#[allow(clippy::too_many_arguments)]
pub(crate) fn layer_of<'a, 'graph: 'a, 'tier: 'a, L: Language>(
    (shapes, builtins, spellings): (
        &'a Shapes<'a>,
        &'graph crate::knot::KBuiltins<'graph, 'graph>,
        Spellings<'_>,
    ),
    (lay, scratch): (impl crate::memory::Lays<'a>, &Bump),
    (facts, candidates, witnesses): (Facts<'a>, &[&'a BuildLayer<'a>], bool),
    (load, requests): (&LoadFacts<'_>, bool),
    ask: &Ask,
    tree_of: impl Fn(crate::values::ContentDigest) -> Option<&'a Tree<'a>>,
    types: &Chain<'tier, '_, '_, '_>,
    resting: &mut dyn FnMut(),
    named: &mut BumpVec<'_, DeclaredType<Parametric>>,
) -> Option<Resumed<'a>> {
    let mut staged = staged(lay, scratch, ask, tree_of)?;
    // The layer binds the asked names alone; the walk's store binds the parent's beneath them.
    let own = Binding::laid(lay, &mut staged);
    facts.each_bound(&mut |unknown, bound| {
        if own.bound(unknown).is_none() {
            staged.push((unknown, bound));
        }
    });
    let binding = Binding::laid(lay, &mut staged);
    let base = facts.whole(types);
    // The binding's types resolve before a question is asked of them.
    resting();
    let levels = Levels::of(scratch, shapes, binding);
    let keeps = resume::kept_by((types, scratch, spellings), facts, &levels);
    let candidates = candidates.iter().map(|each| (*each, *each));
    let resuming = match resume::chosen(candidates, &keeps, own) {
        Some((witnessed, kept)) => Resuming::Over(witnessed, kept),
        None if witnesses
            && resume::witnesses()
            && (own.entries().iter()).any(|(_, bound)| matches!(bound, Bound::Type(_))) =>
        {
            let mut names = BumpVec::new_in(scratch);
            names.extend(
                (own.entries().iter()).filter(|(_, bound)| matches!(bound, Bound::Type(_))),
            );
            Resuming::Witnessing {
                spine: facts,
                witness: Binding::laid(lay, &mut names),
                site: EventSite::entry(ask.body),
            }
        }
        None => Resuming::Closed,
    };
    let resumed = match resuming {
        Resuming::Closed => Resumed {
            witnessed: None,
            layer: walked_layer::<L, _>(
                (shapes, builtins, spellings),
                (lay, scratch),
                (base, facts.innermost()),
                (binding, own),
                types,
                ask.body,
                named,
            ),
        },
        _ => {
            match resume::resumed::<L>(
                (shapes, builtins, spellings),
                (lay, scratch, &mut Flattened::new(scratch)),
                facts,
                (binding, own),
                (resuming, false),
                types,
                ask.body,
            ) {
                Ok(resumed) => {
                    let mut name = |kt| named.push(kt);
                    resumed.layer.each_own_type(&mut name);
                    (resumed.witnessed).inspect(|witnessed| witnessed.each_own_type(&mut name));
                    resumed
                }
                // The refusal's types rest as its records' would.
                Err(error) => {
                    let refusal = relaid_error(lay, scratch, &error);
                    refusal.each_named(&mut |kt| named.push(kt));
                    Resumed {
                        witnessed: None,
                        layer: BuildLayer::refused(lay, facts.innermost(), own, refusal),
                    }
                }
            }
        }
    };
    if !requests || BuildLayer::refusal(&resumed.layer).is_some() {
        return Some(resumed);
    }
    let owner = Some((ask.body, ask));
    let requests = requests_of(lay, scratch, load, owner, types, &mut |kt| named.push(kt));
    Some(Resumed {
        layer: resumed.layer.requesting(requests),
        ..resumed
    })
}

/// What a miss settles for `ask` over `parent`, the layers below, laid in `bump`: `L`'s walk rooted
/// at the asked body under the asked binding and `parent`'s, each record kept in a layer over
/// `parent`, over the shapes of `parent`'s base — the load's, or a composition's. It interns
/// through a chain of its own over the agent tier, laid in `bump` too, and under the root adopts
/// every type the records name into the agent tier. Under a composition it rests nothing there:
/// the chain mints the terms of the layers below first, and the settlement keeps a term of each
/// type its records name past the agent tier. A refusal its walk reaches is kept worded as the
/// load words it. A module's tree is found in `parent`, the load, or a `pinned` composition:
/// `None` where none holds it; otherwise `rest`, handed the walk's chain, rests the binding's
/// types before the walk reads them: in the agent tier under the root, in that chain under a
/// composition. The layer is [`layer_of`]'s, the load's step: over the load it carries its call
/// sites' requests, whose types rest as its records' do; under a composition it carries none.
/// Under the root or a load layer it reads over a witnessed layer the load laid for the body
/// beneath `parent` ([`LoadFacts::witnessed`]), where the binding keeps its questions.
pub(crate) fn settled<'a, 'graph, L: Language>(
    program: &Program<'graph>,
    bump: &'a Bump,
    parent: Facts<'a>,
    pinned: &[&'a Composition<'a>],
    ask: &Ask,
    rest: &mut dyn FnMut(Option<&Chain<'_, '_, '_, '_>>),
    _: &'a &'graph (),
) -> Option<BuildLayer<'a>> {
    let scratch = Bump::new();
    let shapes = parent.shapes();
    let composed = parent.composition().is_some();
    let load: &'a crate::scope::LoadFacts<'a> = program.facts();
    let tree_of = |content| {
        (parent.tree_of(content))
            .or_else(|| composed.then(|| load.tree_of(content)).flatten())
            .or_else(|| pinned.iter().find_map(|pin| pin.facts().tree_of(content)))
    };
    let tiers = program.tiers();
    let types = tiers.settling(bump);
    let mut named: BumpVec<'_, DeclaredType<Parametric>> = BumpVec::new_in(&scratch);
    // Under the root or a load layer, the witnessed layers the load laid for the body beneath it,
    // each once; a settlement witnesses nothing, as no later miss could find its own.
    let mut candidates: BumpVec<'_, &'a BuildLayer<'a>> = BumpVec::new_in(&scratch);
    if !composed {
        for witnessed in load.witnessed(parent.top(), ask.body) {
            if !(candidates.iter()).any(|held| std::ptr::addr_eq(*held, witnessed)) {
                candidates.push(witnessed);
            }
        }
    }
    // A composition's body indexes its own shapes, never the load's slot maps.
    let settlement = layer_of::<L>(
        (
            shapes,
            program.builtins(),
            Spellings::new(program.symbols()),
        ),
        (bump, &scratch),
        (parent, &candidates, false),
        (load, !composed),
        ask,
        tree_of,
        &types,
        &mut || rest(Some(&types)),
        &mut named,
    )?
    .layer;
    // What a frame under it reads rests in the agent tier under the root, and is termed — a term
    // of each type the agent tier lacks — under a composition.
    let resting = tiers.resting();
    if !composed {
        named.iter().for_each(|kt| resting.adopt_from(&types, *kt));
        return Some(settlement);
    }
    Some(settlement.termed(exported(&types, &resting, &named, bump, &scratch)))
}

/// What each name `ask` lists binds, staged in `scratch` beside its unknown: a type as it is, a
/// module as the tree `tree_of` finds for its content, retyped in `lay` where the ask says so.
/// `None` where `tree_of` finds no tree for a module: the call settles nothing.
pub(crate) fn staged<'a, 'x>(
    lay: impl crate::memory::Lays<'a>,
    scratch: &'x Bump,
    ask: &Ask,
    tree_of: impl Fn(crate::values::ContentDigest) -> Option<&'a Tree<'a>>,
) -> Option<BumpVec<'x, (Unknown, Bound<'a>)>> {
    let mut staged = BumpVec::new_in(scratch);
    for asked in &ask.bound {
        let bound = match *asked {
            Asked::Type { named, .. } => Bound::Type(named),
            Asked::Module {
                carried,
                content,
                retyped,
                ..
            } => {
                let tree = tree_of(content)?;
                Bound::Module {
                    carried,
                    tree: match retyped {
                        true => resident(lay, Tree::retyped_closed(carried, tree)),
                        false => tree,
                    },
                }
            }
        };
        let unknown = Unknown {
            segment: ask.body,
            at: Address::Slot(asked.slot()),
        };
        staged.push((unknown, bound));
    }
    Some(staged)
}

/// The layer `L`'s walk rooted at `body` lays over `base`, the layers below, through `lay`: its
/// store binds `binding`, the layer `own` alone, over `below`, the innermost layer of `base`.
/// The walk interns through `types` — the run's settling chain, or the load's own — in which what
/// its binding names already rests. A refusal is kept worded as the load words it. Every type the
/// layer's records, its table of closed types or its refusal name is handed to `named`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn walked_layer<
    'a,
    'graph: 'a,
    'tier: 'a,
    L: Language,
    B: crate::scope::RecordSource<'a>,
>(
    (shapes, builtins, spellings): (
        &'a Shapes<'a>,
        &'graph crate::knot::KBuiltins<'graph, 'graph>,
        Spellings<'_>,
    ),
    (lay, scratch): (impl crate::memory::Lays<'a>, &Bump),
    (base, below): (B, Option<&'a BuildLayer<'a>>),
    (binding, own): (Binding<'a>, Binding<'a>),
    types: &Chain<'tier, '_, '_, '_>,
    body: crate::scope::Segment,
    named: &mut BumpVec<'_, DeclaredType<Parametric>>,
) -> BuildLayer<'a> {
    let layer = LayerStore::new((lay, scratch, spellings), base, binding);
    let walked = L::facts(shapes, builtins, &layer, types, scratch, body);
    match walked {
        Ok(()) => {
            let (units, closings) = layer.writes().sorted(scratch);
            let mut name = |kt| named.push(kt);
            (units.iter().filter_map(|(_, record)| record.typed())).for_each(
                |(facts, frontier)| {
                    facts.each_type(&mut name);
                    frontier.each_type(&mut name);
                },
            );
            closings.iter().for_each(|(_, (closing, frontier))| {
                closing.each_type(&mut name);
                frontier.each_type(&mut name);
            });
            layer.writes().closed().each_type(&mut name);
            BuildLayer::of(lay, scratch, shapes, below, own, layer.writes())
        }
        // The refusal's types rest as its records' would.
        Err(error) => {
            let refusal = relaid_error(lay, scratch, &error);
            refusal.each_named(&mut |kt| named.push(kt));
            BuildLayer::refused(lay, below, own, refusal)
        }
    }
}

/// Where a frame of a callee is born, as the load can name it.
#[derive(Clone, Copy)]
enum Beneath {
    Root,
    Body(Segment),
    Code,
}

/// The innermost callable keying on anything that the birth of `callee` lies within — whose
/// frame's layer a closure of `callee` holds — else the root; `Code` inside a quote's code.
fn beneath(load: &LoadFacts<'_>, keyed: impl Fn(Segment) -> bool, callee: Segment) -> Beneath {
    let shapes = load.shapes();
    let out = |at: Segment| shapes.owner(shapes.parent(at).expect("a callee has an encloser").0);
    let mut at = out(callee);
    loop {
        match load.shapes().shape_at(at).kind() {
            ShapeKind::Program => return Beneath::Root,
            ShapeKind::Code => return Beneath::Code,
            _ if keyed(at) => return Beneath::Body(at),
            _ => at = out(at),
        }
    }
}

/// A layer the load's pass laid: beneath which layer, under which key, of which body, and the
/// first call in text order whose binding it is.
struct Laid<'graph> {
    parent: Option<u32>,
    key: LayerKey,
    body: Segment,
    layer: &'graph BuildLayer<'graph>,
    first: &'graph SlotMap<'graph>,
}

/// Where `map`'s call is written, in text order.
fn text_order(map: &SlotMap<'_>) -> (u32, u32) {
    (map.call.file.0, map.call.span.start)
}

/// The load over `folded`, its folded facts, with the layers its trace names: the base carrying
/// the requests of the sites that pass load content alone, then each site the trace names laid
/// find-or-lay under each layer laid so far, the base first — an all-content site beneath the
/// layer of the callable its callee is born under, a site passing on a slot of its owning body
/// under the layer it reads its request off, beneath its callee's birth body's layer on that
/// layer's chain. Each layer is [`layer_of`]'s, its requests with it, interned through `types`,
/// the load's own chain, so its types rest in the root, and laid through `writer` in program
/// storage, a parent before its children, over a witnessed layer laid for the same body beneath
/// the same layer where the binding keeps its questions. A layer whose walk refused refuses the
/// load, naming the first call in text order whose binding it is.
pub(crate) fn traced<'graph, L: Language>(
    (shapes, builtins, spellings): (
        &'graph Shapes<'graph>,
        &'graph crate::knot::KBuiltins<'graph, 'graph>,
        Spellings<'_>,
    ),
    types: &Chain<'graph, 'graph, 'graph, '_>,
    writer: Writer<'graph, '_>,
    scratch: &Bump,
    folded: &'graph LoadFacts<'graph>,
) -> Result<&'graph LoadFacts<'graph>, ShapeError<'graph>> {
    let requests = requests_of(writer, scratch, folded, None, types, &mut |_| {});
    let load = folded.rebased(resident(writer, folded.base_layer().requesting(requests)));
    let written = Written::of(&load);
    let keyed = |segment| written.body(writer, segment).get().keys.is_some();
    // Each call site the trace names, beside its owning body and where its callee is born, grouped
    // by the body whose layers it is laid under — its callee's birth body for an all-content site
    // (`None`, the base, for the root), its owner for a site passing on a slot — in body order. A
    // site passing on a slot of an open owner is laid under its owner's first layer alone, so a
    // settlement of another binding reads the witnessed layer that one laid.
    let mut targets = BumpVec::new_in(scratch);
    for shape in shapes.bodies() {
        let owner = shape.segment();
        for map in load.slot_maps(owner) {
            let at = load.shapes().shape_at(map.at);
            match load.trace(at, map.site) {
                Some(CallTrace::Named { .. }) => {}
                Some(CallTrace::Untraced(Untraced::Caller(_))) if map.passes_on() => {}
                _ => continue,
            }
            match beneath(&load, keyed, map.callee) {
                Beneath::Code => {}
                // A callee born off the site's lexical chain.
                Beneath::Body(birth) if !map.passes_on() && !shapes.encloses(birth, map.at) => {}
                born => {
                    let group = match born {
                        _ if map.passes_on() => Some(owner),
                        Beneath::Body(birth) => Some(birth),
                        _ => None,
                    };
                    targets.push((group, map, born));
                }
            }
        }
    }
    targets.sort_by_key(|(group, ..)| *group);
    let mut laid: BumpVec<'_, Laid<'graph>> = BumpVec::new_in(scratch);
    // `laid` by parent and key, each beside its ordinal, sorted.
    let mut found: BumpVec<'_, (Option<u32>, LayerKey, u32)> = BumpVec::new_in(scratch);
    let mut named = BumpVec::new_in(scratch);
    // Each witnessed layer laid so far, beside the parent and body it was laid for.
    let mut witnessed: BumpVec<'_, (Option<u32>, Segment, &'graph BuildLayer<'graph>)> =
        BumpVec::new_in(scratch);
    let mut find_or_lay = |laid: &mut BumpVec<'_, Laid<'graph>>,
                           parent: Option<u32>,
                           request: &SiteRequest<'_>,
                           call: &'graph SlotMap<'graph>| {
        match found.binary_search_by_key(&(parent, request.key), |(at, key, _)| (*at, *key)) {
            Ok(at) => {
                let first = &mut laid[found[at].2 as usize].first;
                if text_order(call) < text_order(first) {
                    *first = call;
                }
            }
            Err(at) => {
                let below = parent.map(|ordinal| laid[ordinal as usize].layer);
                let facts = Facts::under(&load, below);
                let sought = request.sought();
                // The witnessed layers laid for the body over the same layers.
                let mut candidates = BumpVec::new_in(scratch);
                candidates.extend(
                    (witnessed.iter())
                        .filter(|(at, body, _)| (*at, *body) == (parent, sought.ask.body))
                        .map(|(.., layer)| *layer),
                );
                named.clear();
                let Some(resumed) = layer_of::<L>(
                    (shapes, builtins, spellings),
                    (writer, scratch),
                    (facts, &candidates, true),
                    (&load, true),
                    &sought.ask,
                    |content| facts.tree_of(content),
                    types,
                    &mut || {},
                    &mut named,
                ) else {
                    return;
                };
                if let Some(layer) = resumed.witnessed {
                    witnessed.push((parent, sought.ask.body, layer));
                }
                found.insert(at, (parent, request.key, laid.len() as u32));
                laid.push(Laid {
                    parent,
                    key: request.key,
                    body: sought.ask.body,
                    layer: resident(writer, resumed.layer),
                    first: call,
                });
            }
        }
    };
    // A FIFO over the laid layers, the base first; a refused layer lays nothing beneath it.
    let mut under: Option<u32> = None;
    let mut next = 0;
    loop {
        let group = under.map(|at| laid[at as usize].body);
        let from = targets.partition_point(|(each, ..)| *each < group);
        let to = from + targets[from..].partition_point(|(each, ..)| *each == group);
        for &(_, map, born) in &targets[from..to] {
            if !map.passes_on() {
                if let Some(request) = load.base_layer().request(map.at, map.site) {
                    find_or_lay(&mut laid, under, request, map);
                }
                continue;
            }
            // A site passing on a slot of its owner, under a layer of that owner: of an open
            // owner, its first.
            let at = under.expect("a site passing on a slot is grouped under its owner's layers");
            let open = !matches!(
                load.trace(load.shapes().shape_at(map.at), map.site),
                Some(CallTrace::Named { .. })
            );
            let first = (laid.iter()).position(|each| Some(each.body) == group);
            if open && first != Some(at as usize) {
                continue;
            }
            let Some(request) = laid[at as usize].layer.request(map.at, map.site) else {
                continue;
            };
            let parent = match born {
                Beneath::Body(birth) => {
                    let mut chain = std::iter::successors(Some(at), |at| laid[*at as usize].parent);
                    let Some(parent) = chain.find(|at| laid[*at as usize].body == birth) else {
                        continue;
                    };
                    Some(parent)
                }
                _ => None,
            };
            find_or_lay(&mut laid, parent, request, map);
        }
        let Some(at) = (next..laid.len()).find(|at| BuildLayer::refusal(laid[*at].layer).is_none())
        else {
            break;
        };
        under = Some(at as u32);
        next = at + 1;
    }
    let refused = (laid.iter())
        .filter_map(|laid| Some((laid.first, BuildLayer::refusal(laid.layer)?)))
        .min_by_key(|(first, _)| text_order(first));
    if let Some((first, refusal)) = refused {
        return Err(ShapeError::UnderCall {
            call: first.call,
            refusal,
        });
    }
    let table = (laid.iter()).map(|laid| LoadLayer {
        parent: laid.parent,
        key: laid.key,
        body: laid.body,
        layer: laid.layer,
    });
    let table = crate::memory::collect(writer, table);
    Ok(resident(writer, load.laying(writer, scratch, table)))
}

/// The composition of the code `syntax`, composed at run time — binding `parameters`, where it is
/// the untraced body of a callable: its shapes built alone under the program's claims and laid
/// through `brand`, then `L`'s walk rooted at them with nothing bound,
/// its records and folds laid there too, as the load lays the program's. The walk interns through
/// a chain of its own over the agent tier, laid in `bump`; a term of each type the shapes or
/// records name that the agent tier and the root do not hold is laid in `bump` too, for a frame
/// under the composition to mint, so nothing rests in the agent tier. A refusal its code reached —
/// the walk's, or the code's own — is kept worded as the load words it.
pub(crate) fn composed<'a, 'graph, L: Language>(
    program: &Program<'graph>,
    brand: ProgramBrand<'a>,
    bump: &'a Bump,
    (syntax, parameters): (&'a KExpression<'a>, &'a [BinderSymbol]),
    _: &'a &'graph (),
) -> Composition<'a> {
    let scratch = Bump::new();
    let tiers = program.tiers();
    let types = tiers.settling(bump);
    let claims = program.shapes().root().group_frame().claims();
    let builtins = program.builtins();
    let symbols = program.symbols();
    let shapes = Shapes::of_code(
        brand,
        (syntax, parameters),
        claims,
        builtins,
        &types,
        symbols,
        &scratch,
    );
    let writer = brand.writer();
    let spellings = crate::scope::Spellings::new(symbols);
    let store = LoadStore::new(writer, &scratch, (shapes, spellings), Binding::EMPTY);
    let root = shapes.root();
    let walked = L::facts(shapes, builtins, &store, &types, &scratch, root.segment());
    let unfolded = resident(writer, store.facts(&scratch));
    let facts = resident(
        writer,
        unfolded.folded(writer, fold((unfolded, &types), writer, &scratch)),
    );
    let refusal = match walked {
        Err(error) => Some(relaid_error(bump, &scratch, &error)),
        Ok(()) => (facts.typing_refusal(&types, &scratch, root, &mut |_| None))
            .or_else(|| facts.overlap(&types, root))
            .map(|error| relaid_error(bump, &scratch, error)),
    };
    // Each type a frame under the composition reads, as a term where the agent tier lacks it.
    let resting = tiers.resting();
    let mut named: BumpVec<'_, DeclaredType<Parametric>> = BumpVec::new_in(&scratch);
    for shape in shapes.bodies() {
        named.push(DeclaredType::from(shape.code_type()));
    }
    facts.each_type(&mut |kt| named.push(kt));
    refusal.each_named(&mut |kt| named.push(kt));
    let quotes = quote_paths(writer, &scratch, shapes, syntax);
    // The base layer keeps the terms and the refusal.
    let layer = (facts.base_layer())
        .termed(exported(&types, &resting, &named, bump, &scratch))
        .refusing(refusal);
    Composition::new(
        syntax,
        resident(writer, facts.rebased(resident(writer, layer))),
        quotes,
    )
}

/// A term of each type in `named` that `resting` lacks, exported from `types` and laid in `bump`,
/// by digest: what a frame under a composition mints so every type its facts name resolves.
fn exported<'a>(
    types: &Chain<'_, '_, '_, '_>,
    resting: &Chain<'_, '_, '_, '_>,
    named: &[DeclaredType<Parametric>],
    bump: &'a Bump,
    scratch: &Bump,
) -> Keyed<'a, (TypeDigest, Term<'a>)> {
    let mut exported: BumpBackedSet<'_, DeclaredType<Parametric>> = bump_set(scratch);
    let mut terms = BumpVec::new_in(scratch);
    for kt in named.iter() {
        if !resting.resolves(*kt) && exported.insert(*kt) {
            terms.push((kt.digest(), types.export(*kt, resting, bump, scratch)));
        }
    }
    Keyed::sorted(bump, &mut terms)
}
