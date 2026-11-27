//! What a composition's miss builds: the facts of code composed at run time, walked as the load
//! walks the program's ([`composed`]), and the function a call runs ([`called_function`]).
//!
//! A composition's miss builds its code's shapes alone over a copy of its syntax and walks them as
//! the load walks the program's: the language's walk rooted at them with nothing bound, its
//! records and folds laid in the composition's own storage, then its binding graph, whose
//! instances every frame under the composition makes and reads. See
//! [build/README.md](../build/README.md).

use crate::build::LoadStore;
use crate::build::fold;
use crate::build::lay::{BaseInstances, Instances};
use crate::knot::{Function, KValue, Knotted};
use crate::memory::{Bump, BumpBackedSet, BumpVec, ProgramBrand, bump_set, resident};
use crate::parse::KExpression;
use crate::scheduler::ComposedFacts;
use crate::scope::{Composition, Keyed, Named, Shapes, quote_paths, relaid_error};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{DeclaredType, Parametric, Term, TypeDigest};
use crate::values::{Chain, DepotRef};

use super::record::{Language, Program};
use super::substrate::bound_graph;

/// The function a call of `callee` runs, behind every barrier: `None` for a callee that is no
/// function.
pub fn called_function<'graph, 'cell>(
    callee: KValue<'graph, 'cell>,
) -> Option<&'cell Function<'graph, 'cell, Knotted<'graph, 'cell>>> {
    callee.as_callable()?.behind_barriers().function()
}

/// The depot slot `callee`'s function was born under, behind every barrier: where a frame of it
/// runs. The root for a callee that is no function.
pub fn born_under(callee: KValue<'_, '_>) -> DepotRef {
    called_function(callee).map_or(DepotRef::ROOT, |function| function.closure().build())
}

/// The composition of the code `syntax`, composed at run time — binding `parameters`, where it is
/// the untraced body of a callable: its shapes built alone under the program's claims and laid
/// through `brand`, then `L`'s walk rooted at them with nothing bound,
/// its records and folds laid there too, as the load lays the program's. The walk interns through
/// a chain of its own over the agent tier, laid in `bump`; a term of each type the shapes or
/// records name that the agent tier and the root do not hold is laid in `bump` too, for a frame
/// under the composition to mint, so nothing rests in the agent tier. Code the walk admits gets
/// its binding graph, its frozen part laid in `bump`, and the instances of it every frame under the
/// composition makes, lending `home`'s own bump. A refusal its code reached — the walk's, the
/// graph's or the code's own — is kept worded as the load words it.
pub(crate) fn composed<'a, 'graph, L: Language>(
    program: &Program<'graph>,
    brand: ProgramBrand<'a>,
    home: crate::scheduler::ComposedHome<'a>,
    (syntax, parameters): (&'a KExpression<'a>, &'a [BinderSymbol]),
    _: &'a &'graph (),
) -> ComposedFacts<'a> {
    let (scratch, bump) = (Bump::new(), home.bump);
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
    let store = LoadStore::new(writer, &scratch, (shapes, spellings));
    let root = shapes.root();
    let walked = L::facts(shapes, builtins, &store, &types, &scratch);
    let unfolded = resident(writer, store.facts(&scratch));
    let facts = resident(
        writer,
        unfolded.folded(writer, fold(unfolded, writer, &scratch)),
    );
    let refusal = match walked {
        Err(error) => Some(relaid_error(bump, &error)),
        Ok(()) => (facts.typing_refusal(&types, &scratch, root, &mut |_| None))
            .or_else(|| facts.overlap(&types, root))
            .map(|error| relaid_error(bump, error)),
    };
    // Code the walk admits gets its binding graph, which refuses it as the load's refuses a
    // program: a recursion that grows, or a refusal its root instance answers.
    let (binding, refusal) = match refusal {
        Some(refusal) => (None, Some(refusal)),
        None => match bound_graph::<L>(facts, &types, writer, &scratch, symbols, (bump, home.laid))
        {
            Ok(binding) => (Some(binding), None),
            Err(error) => (None, Some(relaid_error(bump, &error))),
        },
    };
    // Each type a frame under the composition reads, as a term where the agent tier lacks it.
    let resting = tiers.resting();
    let mut named: BumpVec<'_, DeclaredType<Parametric>> = BumpVec::new_in(&scratch);
    for shape in shapes.bodies() {
        named.push(DeclaredType::from(shape.code_type()));
    }
    facts.each_type(&mut |kt| named.push(kt));
    // The questions its records store and pose, which a frame under it reads where its instances
    // answer them, and what its binding graph names.
    for (_, (unit, _)) in facts.own_units() {
        (unit.questions.iter()).for_each(|question| question.each_type(&mut |kt| named.push(kt)));
        (unit.posed.iter()).for_each(|posed| posed.question.each_type(&mut |kt| named.push(kt)));
    }
    for (_, (closing, _)) in facts.own_closings() {
        (closing.questions.iter())
            .for_each(|question| question.each_type(&mut |kt| named.push(kt)));
    }
    if let Some(binding) = binding {
        binding.facts.each_type(&mut |kt| named.push(kt));
    }
    refusal.each_named(&mut |kt| named.push(kt));
    let quotes = quote_paths(writer, &scratch, shapes, syntax);
    // The base layer keeps the terms and the refusal.
    let layer = (facts.base_layer())
        .termed(exported(&types, &resting, &named, bump, &scratch))
        .refusing(refusal);
    let facts = resident(writer, facts.rebased(resident(writer, layer)));
    let composition = Composition::new(syntax, facts, quotes);
    // The composition's instances, which every frame under it makes and reads, lend its own bump.
    let instances = binding.map(|binding| {
        let instances = Instances::new(
            binding,
            (facts, spellings),
            (L::rebind, L::instance),
            home.lend,
            home.budget,
        );
        resident(home.lend, instances) as &dyn BaseInstances
    });
    (composition, instances)
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
