//! What a call seeks.
//!
//! A call hands the drain a [`Seek`], whatever its frame's placement: the arena node the callee was
//! born under and, where the callee's body keys a settlement — its body's record carries the key
//! list the fold laid, so the asker reads no shape table — the key of its segment under the content at each keyed name and
//! the image of the types that content carries. A parameter's content is its argument; a `FOR ALL`
//! name's is the call's solution at it, digested as the type value the frame binds: carried by a
//! keyworded call, held by an instance, and solved here, on the asker's region, for a call by
//! name. A call past a header or a barrier of a body keyed on a `FOR ALL` name seeks no
//! settlement, since its frame solves only once the arguments are retyped or have crossed, unless
//! it calls an instance, whose solution holds wherever it is called from.
//!
//! A keyed name holds **load content** where the load could have declared it: a type whose handle
//! rests in the root, a type value naming one, or a module whose content digest names a closed tree
//! the callee's chain holds: under the root, the load's index alone, checked here; under a
//! settlement, resolved by the drain's miss through that settlement. Anything else is content the
//! run produced, and a call keying any seeks nothing, or settles nothing. A miss runs [`settled`]:
//! the language's walk rooted at the callee's body, over a layer on the settlement's parent, the
//! binding's types rested only once its trees resolve, and the walk's types adopted into the agent
//! tier once it ends. A callee born of
//! code composed at run time seeks no settlement yet, and runs under its composition: its frame,
//! born under the root where that composition is gone, bounces with a copy of the syntax its body
//! record's root holds. A composition's miss runs [`composed`]: its code's shapes built alone over
//! that copy and walked as the load walks the program's. See
//! [build/README.md](../build/README.md).

use std::fmt::Write as _;

use crate::build::{Layer, LoadStore, fold, key_of};
use crate::knot::{Function, KValue, Knotted};
use crate::memory::{Bump, BumpBackedSet, BumpVec, ProgramBrand, bump_set, collect, resident};
use crate::parse::ProgramNode;
use crate::scheduler::{Ask, Asked, Seek};
use crate::scope::{
    Address, Binding, Bound, Composition, Facts, Settlement, Shapes, Tree, Unknown, quote_paths,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{DeclaredType, KType, Parametric, TypeNode};
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

/// What the call `birth` seeks: the arena node its callee was born under — the root for a birth
/// that is no call or a callee that is no function — and the settlement under it its body keys,
/// if the asker can read its content at every keyed name. `types` is the asker's evaluation
/// chain, which reads the arguments; `region` its region's, where a call by name's solution is
/// solved.
pub fn seek<'graph>(
    birth: &KBirth<'graph, '_>,
    types: &Chain<'_, '_, '_, '_>,
    region: &Chain<'_, '_, '_, '_>,
) -> Seek<'graph> {
    let function = match birth {
        KBirth::Call { callee, .. } => called_function(*callee),
        _ => None,
    };
    Seek {
        parent: function.map_or(ArenaRef::ROOT, |function| function.closure().build()),
        settlement: settlement(birth, types, region),
    }
}

/// The key, binding image and ask of the settlement the call `birth` seeks under its callee's
/// arena node: `None` for a birth that is no call, a callee that is no function, a body that keys
/// no settlement, and a call whose content at a keyed name the asker cannot read or is not load
/// content.
fn settlement<'graph>(
    birth: &KBirth<'graph, '_>,
    types: &Chain<'_, '_, '_, '_>,
    region: &Chain<'_, '_, '_, '_>,
) -> Option<(crate::values::SettlementKey, BindingImage<'graph>, Ask)> {
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
    // A body of composed code lies in its composition's shapes, where nothing settles yet.
    if body.root.is_some() {
        return None;
    }
    let keyed = body.keys?;
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
    let mut binding = smallvec::SmallVec::new();
    let mut bound = smallvec::SmallVec::new();
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
            bound.push((slot, Asked::Type(solved)));
            continue;
        }
        let argument = arguments.field(name.symbol(), types, &scratch)?.value();
        let digest = argument.digest(types, &scratch);
        let asked = match (argument.as_type(), argument.as_module()) {
            (Some(named), _) if region.in_root(named.handle()) => Asked::Type(named.handle()),
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
                    carried,
                    content,
                    retyped: carried != own,
                }
            }
            _ => return None,
        };
        digests.push(digest);
        binding.push(KeyedTypes::of(&argument));
        bound.push((slot, asked));
    }
    let key = key_of(body.segment, digests.into_iter());
    let ask = Ask {
        body: body.segment,
        bound,
    };
    Some((key, BindingImage::of(binding, program.tiers(), types), ask))
}

/// What a miss settles for `ask` over `parent`, the layers below, laid in `bump`: `L`'s walk rooted
/// at the asked body under the asked binding and `parent`'s, each record kept in a layer over
/// `parent`. It interns through a chain of its own over the agent tier, laid in `bump` too, and
/// adopts every type the records name into the agent tier. A refusal its walk reaches is kept
/// worded as the load words it. `None` where a module the binding names has no tree in `parent`;
/// otherwise `rest` rests the binding's types before the walk reads them.
pub(crate) fn settled<'a, 'graph, L: Language>(
    program: &Program<'graph>,
    bump: &'a Bump,
    parent: Facts<'a>,
    ask: &Ask,
    rest: &mut dyn FnMut(),
    _: &'a &'graph (),
) -> Option<Settlement<'a>> {
    let scratch = Bump::new();
    let shapes = program.shapes();
    let mut staged = BumpVec::new_in(&scratch);
    for (slot, asked) in &ask.bound {
        let bound = match *asked {
            Asked::Type(named) => Bound::Type(named),
            Asked::Module {
                carried,
                content,
                retyped,
            } => {
                let tree = parent.tree_of(content)?;
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
            at: Address::Slot(*slot),
        };
        staged.push((unknown, bound));
    }
    // The parent's binding stands beneath the asked one.
    for below in parent.binding().entries() {
        if !staged.iter().any(|(own, _)| *own == below.0) {
            staged.push(*below);
        }
    }
    rest();
    let binding = Binding::laid(bump, &mut staged);
    let layer = Layer::new(bump, &scratch, parent, binding);
    let tiers = program.tiers();
    let types = tiers.settling(bump);
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
            let mut adopt = |kt| resting.adopt_from(&types, kt);
            units
                .iter()
                .for_each(|(_, (facts, _))| facts.each_type(&mut adopt));
            closings
                .iter()
                .for_each(|(_, (closing, _))| closing.each_type(&mut adopt));
            Settlement::of(bump, &scratch, shapes, below, binding, layer.writes())
        }
        Err(error) => {
            let mut text = Text(BumpVec::new_in(&scratch));
            write!(text, "{}", error.display(program.symbols(), &types))
                .expect("a refusal renders");
            let refusal = bump.alloc_str(std::str::from_utf8(&text.0).expect("rendered as UTF-8"));
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
    let rendered = |text: std::fmt::Arguments<'_>| {
        let mut rendered = Text(BumpVec::new_in(&scratch));
        rendered.write_fmt(text).expect("a refusal renders");
        &*bump.alloc_str(std::str::from_utf8(&rendered.0).expect("rendered as UTF-8"))
    };
    let refusal = match walked {
        Err(error) => Some(rendered(format_args!("{}", error.display(symbols, &types)))),
        Ok(()) => (facts.typing_refusal(root).or_else(|| facts.overlap(root)))
            .map(|error| rendered(format_args!("{}", error.display(symbols, &types)))),
    };
    // Each type a frame under the composition reads, as a term where the agent tier lacks it.
    let resting = tiers.resting();
    let mut named: BumpVec<'_, DeclaredType<Parametric>> = BumpVec::new_in(&scratch);
    for shape in shapes.bodies() {
        named.push(DeclaredType::from(shape.code_type()));
    }
    facts.each_type(&mut |kt| named.push(kt));
    let mut exported: BumpBackedSet<'_, DeclaredType<Parametric>> = bump_set(&scratch);
    let mut terms = BumpVec::new_in(&scratch);
    for kt in named.iter() {
        if !resting.resolves(*kt) && exported.insert(*kt) {
            terms.push(types.export(*kt, &resting, bump, &scratch));
        }
    }
    let quotes = quote_paths(writer, &scratch, shapes, syntax.reference());
    Composition::new(
        syntax,
        facts,
        collect(bump, terms.iter().copied()),
        refusal,
        quotes,
    )
}

/// Text rendered into a scratch bump.
pub(super) struct Text<'x>(pub(super) BumpVec<'x, u8>);

impl std::fmt::Write for Text<'_> {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.0.extend_from_slice(text.as_bytes());
        Ok(())
    }
}
