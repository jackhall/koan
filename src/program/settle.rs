//! What a call seeks.
//!
//! A call hands the drain a [`Seek`], whatever its frame's placement: the arena node the callee was
//! born under and, where the callee's body keys a settlement — its body's record carries the key
//! list the fold laid, so the asker reads no shape table — the key of its segment under the content at each keyed name and
//! the image of the types that content carries. A parameter's content is its argument; a `FOR ALL`
//! name's is the call's solution at it, digested as the type value the frame binds: carried by a
//! keyworded call, held by an instance, and solved here, on the asker's region, for a call by
//! name. Where the callee's body can birth a callable, the request is laid as the frame's
//! [level] — by a `Shares` frame's asker at the call, or by a `Fresh` frame's first step
//! from the request the drain hands it back — which a callable born under the settlement records
//! in its [stamp](crate::scope::Stamp), so a call of it replays the chain where it was evicted. A call past a header or a barrier of a body keyed on a `FOR ALL` name seeks no
//! settlement, since its frame solves only once the arguments are retyped or have crossed, unless
//! it calls an instance, whose solution holds wherever it is called from.
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
//! program's. See
//! [build/README.md](../build/README.md).

use std::fmt::Write as _;

use smallvec::SmallVec;

use crate::build::{Layer, LoadStore, fold, key_of};
use crate::knot::{Function, KValue, Knotted};
use crate::memory::{Bump, BumpBackedSet, BumpVec, ProgramBrand, Writer, bump_set, resident};
use crate::parse::ProgramNode;
use crate::scheduler::{Ask, Asked, Seek, Sought};
use crate::scope::{
    Address, Binding, Bound, Composition, Facts, Keyed, Level, Settlement, Shapes, Slot, Tree,
    Unknown, quote_paths,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{DeclaredType, KType, Parametric, Term, TypeDigest, TypeNode};
use crate::values::digest::type_digest;
use crate::values::{
    ArenaRef, BindingImage, Chain, KeyedTypes, Knotted as _, Value, retype_target,
};

use super::body::admitted;
use super::bundle::KBirth;
use super::record::{CallKind, Language, Program};

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

/// What the call `birth` seeks: the arena node its callee was born under — the root for a birth
/// that is no call or a callee that is no function — and the settlement under it its body keys,
/// `keys`, the callee's as the caller read them ([`callee_keys`]), if the asker can read its
/// content at every keyed name. `types` is the asker's evaluation chain, which reads the
/// arguments; `region` its region's, where a call by name's solution is solved. Where the callee's
/// body can birth a callable and the birth holds no level yet, the request is laid as its level
/// through `lay` — a `Shares` frame's asker, whose region the frame writes — or, with no writer,
/// handed back to the frame's first step to lay ([`level`]).
pub fn seek<'graph, 'here>(
    birth: &mut KBirth<'graph, 'here>,
    keys: Option<&[(Slot, BinderSymbol)]>,
    lay: Option<Writer<'here, '_>>,
    types: &Chain<'_, '_, '_, '_>,
    region: &Chain<'_, '_, '_, '_>,
) -> Seek<'graph> {
    let (function, program) = match birth {
        KBirth::Call {
            callee, program, ..
        } => (called_function(*callee), Some(*program)),
        _ => (None, None),
    };
    let parent = function.map_or(ArenaRef::ROOT, |function| function.closure().build());
    let births = function.is_some_and(|function| function.body().births);
    let mut hands_back = false;
    let settlement = keys.and_then(|keys| settlement(birth, keys, types, region));
    let settlement = settlement.map(|sought| {
        if births && let KBirth::Call { level: None, .. } = birth {
            match lay {
                Some(writer) => {
                    let laid = level(writer, birth, &sought, types);
                    if let KBirth::Call { level, .. } = birth {
                        *level = Some(laid);
                    }
                }
                None => hands_back = true,
            }
        }
        let tiers = program.expect("a keyed birth is a call").tiers();
        (BindingImage::of(sought.keyed.clone(), tiers, types), sought)
    });
    Seek {
        parent,
        settlement,
        hands_back,
    }
}

#[cfg(test)]
thread_local! {
    /// How many levels were laid.
    pub(crate) static LEVELS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The level of `sought`, the request of the call `birth`, laid through `writer`: the one function
/// that lays a level. A `Shares` frame's asker lays it at the call ([`seek`]), and a `Fresh`
/// frame's first step from the request the drain hands it back, each in the region the frame
/// writes. It pins each keyed module of composed code other than the callee's own, whose root
/// rebuilds that code's composition on a replay; `types` reads those arguments.
pub fn level<'graph, 'here>(
    writer: Writer<'here, '_>,
    birth: &KBirth<'graph, 'here>,
    sought: &Sought,
    types: &Chain<'_, '_, '_, '_>,
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
    Level::laid(writer, sought, &modules)
}

/// The request of the settlement the call `birth` seeks under its callee's arena node, its body
/// keyed on `keyed`: `None` for a birth that is no call, a callee that is no function, and a call
/// whose content at a keyed name the asker cannot read or is not load content.
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
    let under_root = function.closure().build() == ArenaRef::ROOT;
    let scratch = Bump::new();
    // The call's solution of its group, solved at most once and only for a keyed `FOR ALL` name:
    // `None` past a header or a barrier, whose frame solves after retyping or crossing.
    let solve = || -> Option<BumpVec<'_, KType>> {
        let mut solved = BumpVec::new_in(&scratch);
        if let Some(instance) = function.instance() {
            solved.extend_from_slice(instance);
            return Some(solved);
        }
        let node = callee.as_callable()?;
        if node.coerced().is_some() || callee.ktype() != node.declared() {
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
    // The type the frame retypes a module argument at `name` to: its declared parameter type,
    // where that is closed.
    let retyped = |name: crate::symbols::Symbol, argument: &KValue<'graph, '_>| {
        let declared = callee.as_callable()?.declared().view(types);
        let TypeNode::KFunction { params, .. } = declared.body else {
            return None;
        };
        let declared = types.concrete(params.get(name)?)?;
        match retype_target(types, &scratch, argument.ktype(), declared) {
            DeclaredType::Type(carried) => Some(carried),
            DeclaredType::Scheme(_) => None,
        }
    };
    let mut solution = None;
    let mut digests = BumpVec::with_capacity_in(keyed.len(), &scratch);
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
            digests.push(type_digest(solved, kind));
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
        let digest = argument.digest(types, &scratch);
        let asked = match (argument.as_type(), argument.as_module()) {
            (Some(named), _) if region.in_root(named.handle()) => Asked::Type {
                slot,
                named: named.handle(),
            },
            // A module is load content where its bare member is: the type it is seen at retypes
            // the tree the load computed for it. Its tree resolves through the callee's chain —
            // the load alone under the root, and where the drain settles under a settlement.
            (_, Some(_)) => {
                let member = argument.knot_member()?;
                let content = Value::Knotted(member).digest(types, &scratch);
                if under_root {
                    program.facts().tree_of(content)?;
                }
                let (DeclaredType::Type(own), Some(carried)) =
                    (member.ktype(), retyped(name.symbol(), &argument))
                else {
                    return None;
                };
                // The type the frame binds the module at rests beside what it carries.
                binding.push(KeyedTypes {
                    carried: carried.into(),
                    named: None,
                });
                Asked::Module {
                    slot,
                    carried,
                    content,
                    retyped: carried != own,
                    composition: (member.module())
                        .and_then(|module| module.body()?.root)
                        .map(|root| root.digest),
                }
            }
            _ => return None,
        };
        digests.push(digest);
        binding.push(KeyedTypes::of(&argument));
        bound.push(asked);
    }
    let key = key_of(body.base, body.segment, digests.into_iter());
    let ask = Ask {
        body: body.segment,
        bound,
    };
    Some(Sought {
        key,
        ask,
        keyed: binding,
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
/// composition.
pub(crate) fn settled<'a, 'graph, L: Language>(
    program: &Program<'graph>,
    bump: &'a Bump,
    parent: Facts<'a>,
    pinned: &[&'a Composition<'a>],
    ask: &Ask,
    rest: &mut dyn FnMut(Option<&Chain<'_, '_, '_, '_>>),
    _: &'a &'graph (),
) -> Option<Settlement<'a>> {
    let scratch = Bump::new();
    let shapes = parent.shapes();
    let composed = parent.composition().is_some();
    let load: &'a crate::scope::LoadFacts<'a> = program.facts();
    let tree_of = |content| {
        (parent.tree_of(content))
            .or_else(|| composed.then(|| load.tree_of(content)).flatten())
            .or_else(|| pinned.iter().find_map(|pin| pin.facts().tree_of(content)))
    };
    let mut staged = BumpVec::new_in(&scratch);
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
                        true => resident(bump, Tree::retyped(carried, tree)),
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
    // The parent's binding stands beneath the asked one.
    for below in parent.binding().entries() {
        if !staged.iter().any(|(own, _)| *own == below.0) {
            staged.push(*below);
        }
    }
    let binding = Binding::laid(bump, &mut staged);
    let tiers = program.tiers();
    let types = tiers.settling(bump);
    // Every type the layers below and the binding name resolves in the walk's chain before it
    // reads them.
    let layer = Layer::new(bump, &scratch, parent.whole(&types), binding);
    rest(Some(&types));
    let walked = L::facts(
        shapes,
        program.builtins(),
        &layer,
        &types,
        &scratch,
        ask.body,
    );
    let below = parent.top();
    Some(match walked {
        Ok(()) => {
            let resting = tiers.resting();
            let (units, closings) = layer.writes().sorted(&scratch);
            let mut named: BumpVec<'_, DeclaredType<Parametric>> = BumpVec::new_in(&scratch);
            let mut name = |kt| named.push(kt);
            units
                .iter()
                .for_each(|(_, (facts, _))| facts.each_type(&mut name));
            closings
                .iter()
                .for_each(|(_, (closing, _))| closing.each_type(&mut name));
            let settlement = Settlement::of(bump, &scratch, shapes, below, binding, layer.writes());
            if !composed {
                named.iter().for_each(|kt| resting.adopt_from(&types, *kt));
                return Some(settlement);
            }
            // A term of each type a frame under it reads that the agent tier lacks.
            settlement.termed(exported(&types, &resting, &named, bump, &scratch))
        }
        Err(error) => {
            let display = error.display(program.symbols(), &types);
            let refusal = rendered(bump, &scratch, format_args!("{display}"));
            Settlement::refused(below, binding, refusal)
        }
    })
}

/// The composition of the code `syntax`, composed at run time: its shapes built alone under the
/// program's claims and laid through `brand`, then `L`'s walk rooted at them with nothing bound,
/// its records and folds laid there too, as the load lays the program's. The walk interns through
/// a chain of its own over the agent tier, laid in `bump`; a term of each type the shapes or
/// records name that the agent tier and the root do not hold is laid in `bump` too, for a frame
/// under the composition to mint, so nothing rests in the agent tier. A refusal its code reached —
/// the walk's, or the code's own — is kept worded as the load words it.
pub(crate) fn composed<'a, 'graph, L: Language>(
    program: &Program<'graph>,
    brand: ProgramBrand<'a>,
    bump: &'a Bump,
    syntax: ProgramNode<'a>,
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
        syntax.reference(),
        claims,
        builtins,
        &types,
        symbols,
        &scratch,
    );
    let writer = brand.writer();
    let store = LoadStore::new(writer, &scratch, shapes, Binding::EMPTY);
    let root = shapes.root();
    let walked = L::facts(shapes, builtins, &store, &types, &scratch, root.segment());
    let unfolded = resident(writer, store.facts(&scratch));
    let facts = resident(
        writer,
        unfolded.folded(writer, fold(unfolded, writer, &scratch)),
    );
    let refused = |display: std::fmt::Arguments<'_>| rendered(bump, &scratch, display);
    let refusal = match walked {
        Err(error) => Some(refused(format_args!("{}", error.display(symbols, &types)))),
        Ok(()) => (facts
            .typing_refusal(&types, root)
            .or_else(|| facts.overlap(&types, root)))
        .map(|error| refused(format_args!("{}", error.display(symbols, &types)))),
    };
    // Each type a frame under the composition reads, as a term where the agent tier lacks it.
    let resting = tiers.resting();
    let mut named: BumpVec<'_, DeclaredType<Parametric>> = BumpVec::new_in(&scratch);
    for shape in shapes.bodies() {
        named.push(DeclaredType::from(shape.code_type()));
    }
    facts.each_type(&mut |kt| named.push(kt));
    let quotes = quote_paths(writer, &scratch, shapes, syntax.reference());
    Composition::new(
        syntax,
        facts,
        exported(&types, &resting, &named, bump, &scratch),
        refusal,
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

/// `display` rendered into `bump`: a refusal kept worded as the load words it.
fn rendered<'a>(bump: &'a Bump, scratch: &Bump, display: std::fmt::Arguments<'_>) -> &'a str {
    let mut text = Text(BumpVec::new_in(scratch));
    text.write_fmt(display).expect("a refusal renders");
    bump.alloc_str(std::str::from_utf8(&text.0).expect("rendered as UTF-8"))
}

/// Text rendered into a scratch bump.
pub(super) struct Text<'x>(pub(super) BumpVec<'x, u8>);

impl std::fmt::Write for Text<'_> {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.0.extend_from_slice(text.as_bytes());
        Ok(())
    }
}
