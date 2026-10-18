//! Resuming a load under a binding: the facts whose frontier the binding meets re-run, each bound
//! name read as the closed type it binds, and every other fact stands as the load left it.

use crate::parse::ExpressionPart;
use crate::program::{CellSubstrate, Loaded};
use crate::scope::{
    Address, BodyShape, Narrowing, Settled, ShapeKind, Site, Slot, Static, Statics, Unknown,
};
use crate::symbols::{BinderSymbol, SymbolInterner};
use crate::type_lattice::{DeclaredType, KType, display_name};
use crate::values::SpecBounds;

use super::statics::rendered;
use super::{Koan, output};

/// Two overloads of `PICK`, one at `Number` and one at `Str`.
const PICK: &str = "EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                    EXPR #(PICK x :Str) -> Number = #(1)\n";

/// Load `source` and hand `inspect` the load before its root freezes.
fn resumed<R: 'static>(
    source: &str,
    inspect: impl for<'g, 'l, 'door> FnOnce(&Loaded<'g, 'l, 'door, Koan>) -> R,
) -> R {
    let (_, inspected) = CellSubstrate::load_with::<Koan, R>(
        source,
        "<test>",
        8,
        SpecBounds::default(),
        output(),
        |loaded| inspect(&loaded),
    )
    .unwrap_or_else(|error| panic!("the program loads: {error}"));
    inspected
}

/// The slot `name` of `shape` binds, a registration's aside.
fn slot(symbols: &SymbolInterner, shape: &BodyShape<'_>, name: &str) -> Slot {
    (0..shape.slots())
        .map(|at| Slot(at as u32))
        .find(|at| {
            let named = shape.slot_name(*at);
            !matches!(named, BinderSymbol::Registration(_))
                && symbols.display(named.symbol()).to_string() == name
        })
        .unwrap_or_else(|| panic!("`{name}` is bound here"))
}

/// The unknown the slot `name` of `shape` is.
fn unknown(symbols: &SymbolInterner, shape: &BodyShape<'_>, name: &str) -> Unknown {
    Unknown {
        segment: shape.segment(),
        at: Address::Slot(slot(symbols, shape, name)),
    }
}

/// The one quantified callable body the top level registers.
fn quantified<'graph>(root: &'graph BodyShape<'graph>) -> &'graph BodyShape<'graph> {
    let mut found = (root.nested_shapes().iter()).filter(|(_, nested)| {
        nested.kind() == ShapeKind::Callable && !nested.group_levels().is_empty()
    });
    let (_, body) = found.next().expect("a quantified registration");
    body
}

/// The body the binder `name` of `shape` births.
fn born<'graph>(
    symbols: &SymbolInterner,
    shape: &'graph BodyShape<'graph>,
    name: &str,
) -> &'graph BodyShape<'graph> {
    shape
        .births(slot(symbols, shape, name))
        .expect("the binder births a body")
}

/// `shape`'s cell as `settled` leaves it.
fn statics<'graph>(settled: &Settled<'graph>, shape: &BodyShape<'graph>) -> Statics<'graph> {
    match settled.statics.get(shape.segment()) {
        Some((_, held)) => held.statics,
        None => shape.statics().expect("the load typed the shape"),
    }
}

/// The narrowing of the use `LET name = <use>` in `shape`, read off `statics`.
fn narrowing(
    symbols: &SymbolInterner,
    shape: &BodyShape<'_>,
    statics: Statics<'_>,
    name: &str,
) -> String {
    let Some(ExpressionPart::Expression(node)) = shape.rhs(slot(symbols, shape, name)) else {
        panic!("`{name}` is bound to a node");
    };
    let site = Site::of_node(node.reference());
    let fixed = statics.uses.get(site).map(|(_, fixed)| fixed.narrowing);
    rendered(fixed.unwrap_or(Narrowing::Full))
}

/// The upper end of the binder `name` of `shape`, read off `statics`, rendered.
fn upper<'g, 'l, 'door>(
    loaded: &Loaded<'g, 'l, 'door, Koan>,
    shape: &BodyShape<'_>,
    statics: Statics<'_>,
    name: &str,
) -> String {
    let at = slot(loaded.symbols(), shape, name);
    let DeclaredType::Type(typed) = statics.binders[at.index()] else {
        panic!("`{name}` is no scheme");
    };
    display_name(typed.upper, loaded.types(), loaded.symbols()).to_string()
}

#[test]
fn binding_a_for_all_name_selects_the_overload_at_its_type() {
    let source =
        format!("{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET p = (PICK x)\n  p\n)");
    let (before, after, typed) = resumed(&source, |loaded| {
        let symbols = loaded.symbols();
        let f = quantified(loaded.shape());
        let before = narrowing(symbols, f, f.statics().unwrap(), "p");
        let binding = loaded.binding(&[(unknown(symbols, f, "Elt"), KType::NUMBER)]);
        let settled = loaded
            .resume(&[], binding, false)
            .expect("the resumption refuses nothing");
        let after = narrowing(symbols, f, statics(settled, f), "p");
        (before, after, upper(loaded, f, statics(settled, f), "p"))
    });
    assert_eq!(before, "full");
    assert_eq!(after, "selected");
    assert_eq!(typed, "Str", "the `Number` overload returns `Str`");
}

#[test]
fn binding_a_module_parameter_closes_its_paths() {
    let source = format!(
        "SIG Counter FOR ALL #[Carrier] = \
         #[(VAL zero :Carrier) (VAL succ :(FN :{{x :Carrier}} -> Carrier))]\n\
         {PICK}LET Pinned = :(Counter WITH {{Carrier = Number}})\n\
         LET step = (FN :{{m :Counter}} -> Any = #(\n  LET p = (PICK m.zero)\n  p\n))\n"
    );
    let (before, after) = resumed(&source, |loaded| {
        let symbols = loaded.symbols();
        let root = loaded.shape();
        let Static::Closed(pinned) = root.declared_type(slot(symbols, root, "Pinned")) else {
            panic!("the application declares closed");
        };
        let step = born(symbols, root, "step");
        let before = narrowing(symbols, step, step.statics().unwrap(), "p");
        let binding = loaded.binding(&[(unknown(symbols, step, "m"), pinned)]);
        let settled = loaded
            .resume(&[], binding, false)
            .expect("the resumption refuses nothing");
        (
            before,
            narrowing(symbols, step, statics(settled, step), "p"),
        )
    });
    assert_eq!(before, "full");
    assert_eq!(after, "selected");
}

#[test]
fn binding_a_name_no_fact_reads_re_runs_only_its_own() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt Key] #(WRAP x :Elt) -> Any = #(\n  LET p = (PICK x)\n  p\n)"
    );
    let reran = resumed(&source, |loaded| {
        let f = quantified(loaded.shape());
        let binding = loaded.binding(&[(unknown(loaded.symbols(), f, "Key"), KType::NUMBER)]);
        let settled = loaded
            .resume(&[], binding, false)
            .expect("the resumption refuses nothing");
        settled.reran
    });
    assert_eq!(reran, (0, 1), "only `Key`'s own slot re-runs");
}

#[test]
fn an_inner_callable_over_an_outer_name_settles_under_its_binding() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
         LET g = (FN :{{y :Number}} -> Any = #(\n    LET p = (PICK x)\n    p\n  ))\n  g\n)"
    );
    let (before, after) = resumed(&source, |loaded| {
        let symbols = loaded.symbols();
        let f = quantified(loaded.shape());
        let g = born(symbols, f, "g");
        let before = narrowing(symbols, g, g.statics().unwrap(), "p");
        let binding = loaded.binding(&[(unknown(symbols, f, "Elt"), KType::NUMBER)]);
        let settled = loaded
            .resume(&[], binding, false)
            .expect("the resumption refuses nothing");
        (before, narrowing(symbols, g, statics(settled, g), "p"))
    });
    assert_eq!(before, "full");
    assert_eq!(after, "selected");
}

#[test]
fn a_binding_no_overload_admits_refuses_as_the_load_would() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET y = (x + 1)\n  y\n)";
    let refused = resumed(source, |loaded| {
        let f = quantified(loaded.shape());
        let binding = loaded.binding(&[(unknown(loaded.symbols(), f, "Elt"), KType::STR)]);
        match loaded.resume(&[], binding, false) {
            Ok(_) => "settled".to_string(),
            Err(error) => error.display(loaded.symbols(), loaded.types()).to_string(),
        }
    });
    assert!(
        refused.contains("no overload of `_ + _` admits (Str, Number)"),
        "{refused}"
    );
}

#[test]
fn a_type_binder_over_a_bound_name_declares_closed() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
                  NEWTYPE Box = :{v :Elt}\n  (Box {v = x})\n)";
    let (before, after) = resumed(source, |loaded| {
        let f = quantified(loaded.shape());
        let at = slot(loaded.symbols(), f, "Box");
        let before = matches!(f.declared_type(at), Static::Closed(_));
        let binding = loaded.binding(&[(unknown(loaded.symbols(), f, "Elt"), KType::NUMBER)]);
        let settled = loaded
            .resume(&[], binding, false)
            .expect("the resumption refuses nothing");
        let channel = settled
            .channel
            .get(f.segment())
            .map(|(_, held)| held.declared);
        let after = channel
            .and_then(|declared| declared.get(at))
            .map(|(_, typed)| *typed);
        (before, matches!(after, Some(Static::Closed(_))))
    });
    assert!(!before, "the load leaves the binder unknown");
    assert!(after, "the resumption declares it closed");
}

#[test]
fn re_running_every_fact_settles_what_skipping_does() {
    let source =
        format!("{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET p = (PICK x)\n  p\n)");
    let (skipping, everything) = resumed(&source, |loaded| {
        let symbols = loaded.symbols();
        let f = quantified(loaded.shape());
        let binding = loaded.binding(&[(unknown(symbols, f, "Elt"), KType::NUMBER)]);
        let skipping = loaded
            .resume(&[], binding, false)
            .expect("the resumption refuses nothing");
        let everything = loaded
            .resume(&[], binding, true)
            .expect("the resumption refuses nothing");
        let render = |settled| {
            let statics = statics(settled, f);
            (
                narrowing(symbols, f, statics, "p"),
                upper(loaded, f, statics, "p"),
            )
        };
        (render(skipping), render(everything))
    });
    assert_eq!(skipping, everything);
}

#[test]
fn a_check_a_binding_no_longer_settles_is_dropped() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET s :Elt = x\n  s\n)";
    let (load, skipping, everything) = resumed(source, |loaded| {
        let symbols = loaded.symbols();
        let f = quantified(loaded.shape());
        let binding = loaded.binding(&[(unknown(symbols, f, "Elt"), KType::NEVER)]);
        let skipping = loaded
            .resume(&[], binding, false)
            .expect("the resumption refuses nothing");
        let everything = loaded
            .resume(&[], binding, true)
            .expect("the resumption refuses nothing");
        let settled = |settled| statics(settled, f).settled.len();
        let load = f.statics().map(|held| held.settled.len());
        (load, settled(skipping), settled(everything))
    });
    assert_eq!(load, Some(1));
    assert_eq!((skipping, everything), (0, 0));
}

#[test]
fn a_binding_that_starves_a_refused_check_vacates_it() {
    let source = "EXPR FOR ALL #[Elt Key] #(WRAP x :Elt AND k :Key) -> Any = #(\n  \
                  LET s0 = (x :! Key)\n  x\n)";
    let (half, whole) = resumed(source, |loaded| {
        let symbols = loaded.symbols();
        let f = quantified(loaded.shape());
        let (elt, key) = (unknown(symbols, f, "Elt"), unknown(symbols, f, "Key"));
        let half = loaded.resume(&[], loaded.binding(&[(key, KType::NEVER)]), false);
        let half = half
            .map(|_| ())
            .map_err(|error| error.display(symbols, loaded.types()).to_string());
        let both = loaded.binding(&[(elt, KType::NEVER), (key, KType::NEVER)]);
        let whole = loaded
            .resume(&[], both, false)
            .expect("the resumption refuses nothing");
        (half, upper(loaded, f, statics(whole, f), "s0"))
    });
    let refused = half.expect_err("binding `Key` alone refuses the check");
    assert!(
        refused.contains("this value is Any, which can never satisfy its ascription Never"),
        "{refused}"
    );
    assert_eq!(whole, "Never");
}

#[test]
fn a_resumption_extends_the_one_before_it() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt Key] #(WRAP x :Elt AND k :Key) -> Any = #(\n  \
         LET p = (PICK x)\n  LET q = (PICK k)\n  p\n)"
    );
    let (first, second) = resumed(&source, |loaded| {
        let symbols = loaded.symbols();
        let f = quantified(loaded.shape());
        let (elt, key) = (unknown(symbols, f, "Elt"), unknown(symbols, f, "Key"));
        let once = loaded.binding(&[(elt, KType::NUMBER)]);
        let first = loaded
            .resume(&[], once, false)
            .expect("the resumption refuses nothing");
        let both = loaded.binding(&[(elt, KType::NUMBER), (key, KType::STR)]);
        let second = loaded
            .resume(&[first], both, false)
            .expect("the resumption refuses nothing");
        let render = |settled| {
            let statics = statics(settled, f);
            (
                narrowing(symbols, f, statics, "p"),
                narrowing(symbols, f, statics, "q"),
            )
        };
        (render(first), render(second))
    });
    assert_eq!(first, ("selected".to_string(), "full".to_string()));
    assert_eq!(second, ("selected".to_string(), "selected".to_string()));
}

/// The laws over drawn [`Resumable`] programs, each fact rendered by names — never by an index or
/// a source position — so a resumption and a restart render alike where they agree.
mod laws {
    use proptest::prelude::*;

    use crate::memory::Bump;
    use crate::parse::{ExpressionPart, KExpression};
    use crate::program::{CellSubstrate, LoadError, Loaded};
    use crate::scope::{
        Address, Binding, BodyShape, CandidateList, Narrowing, Settled, ShapeKind, Site, Slot,
        Static, Statics, Target, Unknown, each_part,
    };
    use crate::symbols::{BinderSymbol, SymbolInterner};
    use crate::type_lattice::{
        DeclaredType, Habitat, Interval, KType, Parametric, Side, TypeChain, Variable, Verdict,
        display_name, fits, read_through, substitute_levels,
    };
    use crate::values::SpecBounds;
    use crate::values::tests::reading;

    use super::super::generate::{Bound, Resumable, resumable};
    use super::super::{Koan, output};
    use super::resumed;

    /// `SUBJECT`'s body: the last callable the top level births.
    fn subject<'graph>(root: &'graph BodyShape<'graph>) -> &'graph BodyShape<'graph> {
        (root.nested_shapes().iter().rev())
            .find(|(_, nested)| nested.kind() == ShapeKind::Callable)
            .map(|(_, nested)| *nested)
            .expect("the program registers `SUBJECT`")
    }

    /// The slot `name` of `shape` binds, a registration's aside.
    fn slot(symbols: &SymbolInterner, shape: &BodyShape<'_>, name: &str) -> Option<Slot> {
        (0..shape.slots()).map(|at| Slot(at as u32)).find(|at| {
            let named = shape.slot_name(*at);
            !matches!(named, BinderSymbol::Registration(_))
                && symbols.display(named.symbol()).to_string() == name
        })
    }

    /// The binding `bound` gives, read off the load: each unknown at its slot of `SUBJECT`, each
    /// type off the alias the program declares it under.
    fn binding<'g>(loaded: &Loaded<'g, '_, '_, Koan>, bound: &Bound) -> Binding<'g> {
        let (symbols, root) = (loaded.symbols(), loaded.shape());
        let body = subject(root);
        let mut staged = Vec::new();
        for (name, _) in bound.entries() {
            let alias = slot(symbols, root, &Resumable::declared(name)).expect("the alias");
            let Static::Closed(typed) = root.declared_type(alias) else {
                panic!("a bound type declares closed");
            };
            let at = slot(symbols, body, name).expect("the unknown is `SUBJECT`'s");
            let unknown = Unknown {
                segment: body.segment(),
                at: Address::Slot(at),
            };
            staged.push((unknown, typed));
        }
        loaded.binding(&staged)
    }

    /// `shape`'s cell, newest first through `stack`, else the load's.
    fn statics_in<'g>(stack: &[&Settled<'g>], shape: &BodyShape<'g>) -> Option<Statics<'g>> {
        (stack.iter())
            .find_map(|settled| {
                settled
                    .statics
                    .get(shape.segment())
                    .map(|(_, held)| held.statics)
            })
            .or_else(|| shape.statics())
    }

    fn interval<'run, H: Habitat<'run>>(
        typed: Interval,
        types: &TypeChain<'run, H>,
        symbols: &SymbolInterner,
    ) -> String {
        format!(
            "[{}, {}]",
            display_name(typed.lower, types, symbols),
            display_name(typed.upper, types, symbols)
        )
    }

    /// A narrowing by each candidate's index on its use's list.
    fn narrowed(narrowing: Narrowing<'_>, list: &CandidateList<'_>) -> String {
        let index = |coordinate| {
            (list.candidates.iter())
                .position(|held| *held == crate::scope::Candidate::One(coordinate))
                .unwrap_or(usize::MAX)
        };
        match narrowing {
            Narrowing::Full => "full".to_string(),
            Narrowing::Kept(kept) => kept.iter().fold("kept".to_string(), |text, (at, verdict)| {
                let at = match at {
                    crate::scope::Candidate::One(coordinate) => index(*coordinate),
                    crate::scope::Candidate::Spread(_) => usize::MAX,
                };
                format!("{text} {at}:{verdict:?}")
            }),
            Narrowing::Selected(coordinate) => format!("selected {}", index(coordinate)),
        }
    }

    /// Each value part of `statement` that `statics` types, by its index among those, beside its
    /// site; and each use's node site.
    fn parts<'g>(statement: &'g KExpression<'g>, statics: Statics<'_>) -> Vec<Site> {
        let mut sites = Vec::new();
        each_part(statement, |part| {
            if statics.parts.get(Site::of(part)).is_some() {
                sites.push(Site::of(part));
            }
        });
        sites
    }

    /// Every fact of `shape` and the shapes nested in it, rendered by names under `path`.
    fn render<'g, 'run, H: Habitat<'run>>(
        shape: &'g BodyShape<'g>,
        stack: &[&Settled<'g>],
        types: &TypeChain<'run, H>,
        symbols: &SymbolInterner,
        path: &str,
        out: &mut Vec<String>,
    ) {
        let Some(held) = statics_in(stack, shape) else {
            out.push(format!("{path}: untyped"));
            return;
        };
        for at in 0..shape.slots() {
            let name = shape.slot_name(Slot(at as u32));
            let BinderSymbol::Value(_) = name else {
                continue;
            };
            let typed = match held.binders[at] {
                DeclaredType::Type(typed) => interval(typed, types, symbols),
                DeclaredType::Scheme(scheme) => display_name(scheme, types, symbols).to_string(),
            };
            out.push(format!(
                "{path} {} = {typed}",
                symbols.display(name.symbol())
            ));
        }
        for (index, statement) in shape.body().iter().enumerate() {
            let typed = interval(held.statements[index], types, symbols);
            out.push(format!("{path} #{index} = {typed}"));
            for (at, site) in parts(statement, held).into_iter().enumerate() {
                let (_, typed) = held.parts.get(site).expect("the part is typed");
                out.push(format!(
                    "{path} #{index}.{at} = {}",
                    interval(*typed, types, symbols)
                ));
            }
            let mut uses = vec![Site::of_node(statement)];
            each_part(statement, |part| {
                if let ExpressionPart::Expression(node) = part {
                    uses.push(Site::of_node(node.reference()));
                }
            });
            for (at, site) in uses.into_iter().enumerate() {
                if let (Some((_, fixed)), Some((_, list))) =
                    (held.uses.get(site), shape.candidate_lists().get(site))
                {
                    out.push(format!(
                        "{path} #{index} use {at}: {}",
                        narrowed(fixed.narrowing, list)
                    ));
                }
            }
            let settled = (held.settled.iter())
                .filter(|site| crate::scope::source_of(statement, **site).is_some())
                .count();
            out.push(format!("{path} #{index} settled {settled}"));
        }
        for (index, (_, nested)) in shape.nested_shapes().iter().enumerate() {
            render(
                nested,
                stack,
                types,
                symbols,
                &format!("{path}/{index}"),
                out,
            );
        }
    }

    /// A refusal's message, its location dropped.
    fn unlocated(message: &str) -> String {
        match message.split_once(": ") {
            Some((at, rest)) if at.starts_with('<') => rest.to_string(),
            _ => message.to_string(),
        }
    }

    /// What resuming `case`'s load under each of `bindings` in turn renders: each extending the
    /// one before it, and `everything` re-running every fact.
    fn resume_rendered(
        case: &Resumable,
        bindings: &[Bound],
        everything: bool,
    ) -> Result<Vec<String>, String> {
        resumed(&case.render(), |loaded| {
            let mut stack: Vec<&Settled<'_>> = Vec::new();
            for bound in bindings {
                let binding = binding(loaded, bound);
                match loaded.resume(&stack, binding, everything) {
                    Ok(settled) => stack.insert(0, settled),
                    Err(error) => {
                        let shown = error.display(loaded.symbols(), loaded.types()).to_string();
                        return Err(unlocated(&shown));
                    }
                }
            }
            let mut out = Vec::new();
            render(
                subject(loaded.shape()),
                &stack,
                loaded.types(),
                loaded.symbols(),
                "",
                &mut out,
            );
            Ok(out)
        })
    }

    /// What a plain load of `source` renders, or its refusal.
    fn loaded_rendered(source: &str) -> Result<Vec<String>, String> {
        let loaded =
            CellSubstrate::load::<Koan>(source, "<test>", 8, SpecBounds::default(), output());
        let mut substrate = match loaded {
            Ok(substrate) => substrate,
            Err(LoadError::Shape { rendered }) => return Err(unlocated(&rendered)),
            Err(error) => panic!("the restart parses: {error}\n{source}"),
        };
        substrate.with(|running| {
            let program = running.program();
            let bump = Bump::new();
            let types = reading(program.root(), &bump);
            let mut out = Vec::new();
            render(
                subject(program.shape()),
                &[],
                &types,
                program.symbols(),
                "",
                &mut out,
            );
            Ok(out)
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: crate::tests::case_share(1, 4),
            ..ProptestConfig::default()
        })]

        /// A resumption is a restart: resuming the load under a binding renders every fact of
        /// `SUBJECT`'s body as a load of the program with the binding written in does, refusals
        /// included. So a resumption's verdicts are a load's, which hold at every run — the
        /// verdict law the debug net checks for each load.
        #[test]
        fn a_resumption_is_a_restart(case in resumable()) {
            let resumed = resume_rendered(&case, std::slice::from_ref(&case.bound), false);
            let restart = case.restart(&case.bound);
            prop_assert_eq!(resumed, loaded_rendered(&restart), "in\n{}\nrestarted as\n{}", case.render(), restart);
        }

        /// Resumptions are confluent: reaching a binding in two steps, either half first, settles
        /// what reaching it in one does. A refusal on the way agrees with a binding that leaves
        /// its site vacuous, as the monotone law reads one typed `Never`.
        #[test]
        fn resumptions_are_confluent(case in resumable(), split in any::<u8>()) {
            let names: Vec<&str> = case.bound.entries().iter().map(|(name, _)| *name).collect();
            let first: Vec<&str> = (names.iter().enumerate())
                .filter(|(at, _)| split & (1 << at) != 0)
                .map(|(_, name)| *name)
                .collect();
            let second: Vec<&str> = names.iter().copied().filter(|name| !first.contains(name)).collect();
            let whole = resume_rendered(&case, std::slice::from_ref(&case.bound), false);
            for half in [&first, &second] {
                let stepped = resume_rendered(&case, &[case.bound.only(half), case.bound.clone()], false);
                match (&stepped, &whole) {
                    (Err(_), Err(_)) => {}
                    (Err(_), Ok(_)) if vacated(&case, half) => {}
                    _ => prop_assert_eq!(&stepped, &whole, "in\n{}\nfirst {:?}", case.render(), half),
                }
            }
        }

        /// Quiescence is exact: re-running every fact settles what re-running only the woken ones
        /// does, and every residual entry waits on an unknown of a shape enclosing it.
        #[test]
        fn quiescence_is_exact(case in resumable()) {
            let skipping = resume_rendered(&case, std::slice::from_ref(&case.bound), false);
            let everything = resume_rendered(&case, std::slice::from_ref(&case.bound), true);
            prop_assert_eq!(skipping, everything, "in\n{}", case.render());
            let foreign = resumed(&case.render(), |loaded| {
                let mut foreign = Vec::new();
                let mut chain = Vec::new();
                residual_outside(loaded.shape(), &mut chain, &mut foreign);
                foreign
            });
            prop_assert!(foreign.is_empty(), "{:?} in\n{}", foreign, case.render());
        }

        /// A resumption is monotone: each fact it re-runs lies within the load's with the binding
        /// substituted, a verdict of *always* stays, a narrowing only shrinks, and a settled site
        /// stays settled. A fact typed `Never` never arrives: it is the bottom, within every fact,
        /// and a statement typed so never runs its uses and checks.
        #[test]
        fn a_resumption_is_monotone(case in resumable()) {
            let broken = resumed(&case.render(), |loaded| monotone(loaded, &case.bound));
            prop_assert!(broken.is_empty(), "{:?} in\n{}", broken, case.render());
        }

        /// A resumption terminates: it re-runs at most the facts its binding wakes, each once.
        #[test]
        fn a_resumption_reruns_only_what_it_wakes(case in resumable()) {
            let (reran, woken) = resumed(&case.render(), |loaded| {
                let binding = binding(loaded, &case.bound);
                let woken = woken(loaded.shape(), binding);
                let reran = loaded.resume(&[], binding, false).map(|settled| settled.reran).ok();
                (reran, woken)
            });
            if let Some((channel, statics)) = reran {
                prop_assert!(channel <= woken.0 && statics <= woken.1, "{:?} > {:?} in\n{}", (channel, statics), woken, case.render());
            }
        }
    }

    /// Whether the refusal met reaching `case`'s binding by `first` and then the rest lies in a
    /// statement the whole binding types `Never`, of `SUBJECT`'s body or a body nested in it: no
    /// operator run reaches it.
    fn vacated(case: &Resumable, first: &[&str]) -> bool {
        resumed(&case.render(), |loaded| {
            let whole = binding(loaded, &case.bound);
            let refused = match loaded.resume(&[], binding(loaded, &case.bound.only(first)), false)
            {
                Ok(settled) => loaded.resume(&[settled], whole, false).err(),
                Err(error) => Some(error),
            };
            let (Some(refused), Ok(settled)) = (refused, loaded.resume(&[], whole, false)) else {
                return false;
            };
            let at = refused.at().span;
            let mut shapes = vec![subject(loaded.shape())];
            while let Some(shape) = shapes.pop() {
                shapes.extend(shape.nested_shapes().iter().map(|(_, nested)| *nested));
                let Some(now) = statics_in(&[settled], shape) else {
                    continue;
                };
                let vacuous =
                    (shape.body().iter().zip(now.statements)).any(|(statement, typed)| {
                        let span = statement.source.span;
                        typed.upper == KType::NEVER.into()
                            && span.start <= at.start
                            && at.end <= span.end
                    });
                if vacuous {
                    return true;
                }
            }
            false
        })
    }

    /// Each residual entry of `shape` and the shapes nested in it that waits on nothing, or on an
    /// unknown no shape enclosing it declares, `chain` the segments enclosing `shape`.
    fn residual_outside(
        shape: &BodyShape<'_>,
        chain: &mut Vec<crate::scope::Segment>,
        out: &mut Vec<String>,
    ) {
        chain.push(shape.segment());
        for (key, frontier) in shape
            .channel_residual()
            .iter()
            .chain(shape.statics_residual().iter())
        {
            if frontier.is_empty()
                || frontier
                    .unknowns()
                    .iter()
                    .any(|unknown| !chain.contains(&unknown.segment))
            {
                out.push(format!("{key:?} waits on {frontier:?}"));
            }
        }
        for (_, nested) in shape.nested_shapes() {
            residual_outside(nested, chain, out);
        }
        chain.pop();
    }

    /// How many facts of `shape` and the shapes nested in it `binding` wakes: the type channel's
    /// and the value channel's.
    fn woken(shape: &BodyShape<'_>, binding: Binding<'_>) -> (u32, u32) {
        let mut counted = (0, 0);
        for (_, frontier) in shape.channel_residual().iter() {
            counted.0 += frontier.meets(binding) as u32;
        }
        for (_, frontier) in shape.statics_residual().iter() {
            counted.1 += frontier.meets(binding) as u32;
        }
        for (_, nested) in shape.nested_shapes() {
            let inner = woken(nested, binding);
            counted.0 += inner.0;
            counted.1 += inner.1;
        }
        counted
    }

    /// Each fact of `SUBJECT`'s body a resumption under `bound` re-runs outside the load's with
    /// the binding substituted, rendered: the binders, statements and parts whose load value names
    /// only lexical variables the binding binds, and every use and settled site.
    fn monotone(loaded: &Loaded<'_, '_, '_, Koan>, bound: &Bound) -> Vec<String> {
        let binding = binding(loaded, bound);
        let Ok(settled) = loaded.resume(&[], binding, false) else {
            return Vec::new();
        };
        let (types, symbols) = (loaded.types(), loaded.symbols());
        let scratch = Bump::new();
        let body = subject(loaded.shape());
        // The levels the binding gives a type, read off `SUBJECT`'s declared variables.
        let mut levels: Vec<Option<KType>> = Vec::new();
        for (level, target) in body.declared_variables() {
            let Target::Local(slot) = target else {
                continue;
            };
            let unknown = Unknown {
                segment: body.segment(),
                at: Address::Slot(*slot),
            };
            if let Some(typed) = binding.bound(unknown) {
                if levels.len() <= *level {
                    levels.resize(*level + 1, None);
                }
                levels[*level] = Some(match body.slot_name(*slot) {
                    BinderSymbol::Type(_) => typed,
                    _ => continue,
                });
            }
        }
        let substituted = |value: Parametric| -> Option<Parametric> {
            let mut named = Vec::new();
            read_through(types, &scratch, value, Side::Above, &mut |variable| {
                if let Variable::Lexical { level, .. } = variable {
                    named.push(level);
                }
                None
            });
            if named
                .iter()
                .any(|level| levels.get(*level).copied().flatten().is_none())
            {
                return None;
            }
            let prefix: Option<Vec<KType>> = levels.iter().copied().collect();
            let bindings = prefix?;
            Some(substitute_levels(types, &scratch, value, &bindings))
        };
        let mut broken = Vec::new();
        // A fact that never arrives is the bottom, within every fact.
        let never = |typed: Interval| typed.upper == KType::NEVER.into();
        let outside = |what: String, load: Interval, now: Interval| -> Option<String> {
            if never(now) {
                return None;
            }
            let (lower, upper) = (substituted(load.lower)?, substituted(load.upper)?);
            let within =
                fits(types, &scratch, lower, now.lower) && fits(types, &scratch, now.upper, upper);
            (!within).then(|| {
                format!(
                    "{what}: {} outside {}",
                    interval(now, types, symbols),
                    interval(Interval { lower, upper }, types, symbols)
                )
            })
        };
        let mut shapes = vec![body];
        while let Some(shape) = shapes.pop() {
            shapes.extend(shape.nested_shapes().iter().map(|(_, nested)| *nested));
            let (Some(load), Some(now)) = (shape.statics(), statics_in(&[settled], shape)) else {
                continue;
            };
            for at in 0..shape.slots() {
                if let (DeclaredType::Type(was), DeclaredType::Type(is)) =
                    (load.binders[at], now.binders[at])
                {
                    broken.extend(outside(format!("binder {at}"), was, is));
                }
            }
            for (index, (was, is)) in load.statements.iter().zip(now.statements).enumerate() {
                broken.extend(outside(format!("statement {index}"), *was, *is));
            }
            for (site, was) in load.parts.iter() {
                if let Some((_, is)) = now.parts.get(*site) {
                    broken.extend(outside(format!("part {site:?}"), *was, *is));
                }
            }
            // A use or a check in a statement that never completes never runs.
            let vacuous = |site: Site| {
                (shape.body().iter().zip(now.statements)).any(|(statement, typed)| {
                    never(*typed) && crate::scope::source_of(statement, site).is_some()
                })
            };
            for (site, fixed) in load.uses.iter() {
                if vacuous(*site) {
                    continue;
                }
                let was = fixed.narrowing;
                let is = now
                    .uses
                    .get(*site)
                    .map_or(Narrowing::Full, |(_, held)| held.narrowing);
                if let Some(why) = widened(was, is) {
                    broken.push(format!("use {site:?}: {why}"));
                }
            }
            for site in load.settled.iter() {
                if now.settled.get(*site).is_none() && !vacuous(*site) {
                    broken.push(format!("settled {site:?} unsettled"));
                }
            }
        }
        broken
    }

    /// Why the narrowing `is` is no shrinking of `was`, where it is not: a candidate it keeps that
    /// `was` dropped, an *always* that fell, or a selection that moved.
    fn widened(was: Narrowing<'_>, is: Narrowing<'_>) -> Option<String> {
        use crate::scope::Candidate;
        let kept = |narrowing: Narrowing<'_>| -> Option<Vec<(Candidate, Option<Verdict>)>> {
            match narrowing {
                Narrowing::Full => None,
                Narrowing::Kept(kept) => Some(
                    kept.iter()
                        .map(|(at, verdict)| (*at, Some(*verdict)))
                        .collect(),
                ),
                Narrowing::Selected(at) => Some(vec![(Candidate::One(at), None)]),
            }
        };
        match (was, is) {
            (Narrowing::Selected(was), Narrowing::Selected(is)) if was != is => {
                Some("the selection moved".to_string())
            }
            (Narrowing::Selected(_), Narrowing::Selected(_)) => None,
            (Narrowing::Selected(_), _) => Some("a selection widened".to_string()),
            (_, Narrowing::Full) if !matches!(was, Narrowing::Full) => {
                Some("a narrowing widened to full".to_string())
            }
            _ => {
                let (Some(was), Some(is)) = (kept(was), kept(is)) else {
                    return None;
                };
                for (at, verdict) in is.iter() {
                    let Some((_, before)) = was.iter().find(|(held, _)| held == at) else {
                        return Some(format!("{at:?} was dropped"));
                    };
                    if *before == Some(Verdict::Always)
                        && verdict.is_some_and(|now| now != Verdict::Always)
                    {
                        return Some(format!("{at:?} fell from always"));
                    }
                }
                None
            }
        }
    }
}
