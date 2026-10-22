//! Resuming a load under a binding: the units whose frontier the binding meets re-run, each bound
//! name read as the closed type it binds, and every other unit's record stands as the load left it.
//! Each resumption goes through the test door ([`Running::resume`]), after the types' root freezes.

use crate::memory::{Bump, BumpVec, resident};
use crate::parse::ExpressionPart;
use crate::program::tests::stores::module_back;
use crate::program::{CellSubstrate, Running};
use crate::scope::{
    Address, Binding, BodyShape, Bound, LoadFacts, Mint, ShapeKind, Site, Slot, Static, Tree,
    Unknown,
};
use crate::symbols::{BinderSymbol, SymbolInterner};
use crate::type_lattice::{DeclaredType, KType, display_name};
use crate::values::tests::reading;
use crate::values::{Chain, ContentDigest, SpecBounds};

use super::statics::rendered;
use super::{Koan, output};

/// Two overloads of `PICK`, one at `Number` and one at `Str`.
const PICK: &str = "EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                    EXPR #(PICK x :Str) -> Number = #(1)\n";

/// `SIG Counter`, a module signature over a `Carrier` head parameter.
const COUNTER: &str = "SIG Counter FOR ALL #[Carrier] = \
                       #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n";

/// What a test reads a resumption through: the running program, a chain over its frozen root that
/// every resumption interns through, and the bump they lay their records in.
struct Door<'d, 'graph> {
    running: &'d Running<'graph>,
    types: &'d Chain<'d, 'd, 'd, 'd>,
    bump: &'d Bump,
}

impl<'d, 'graph: 'd> Door<'d, 'graph> {
    fn symbols(&self) -> &'graph SymbolInterner {
        self.running.symbols()
    }

    fn root(&self) -> &'graph BodyShape<'graph> {
        self.running.program().shapes().root()
    }

    fn load(&self) -> &'graph LoadFacts<'graph> {
        self.running.program().facts()
    }

    /// `staged` laid down as a binding of types.
    fn binding(&self, staged: &[(Unknown, KType)]) -> Binding<'d> {
        let bound: Vec<_> = (staged.iter())
            .map(|(unknown, typed)| (*unknown, Bound::Type(*typed)))
            .collect();
        self.bound(&bound)
    }

    /// `staged` laid down as a binding.
    fn bound(&self, staged: &[(Unknown, Bound<'d>)]) -> Binding<'d> {
        let mut held = BumpVec::new_in(self.bump);
        held.extend_from_slice(staged);
        Binding::laid(self.bump, &mut held)
    }

    /// What a module unknown binds to: the module `back` read off a run, its carried type over its
    /// content.
    fn module(&self, (carried, digest): (KType, ContentDigest)) -> Bound<'d> {
        Bound::Module {
            carried,
            tree: resident(self.bump, Tree::Digest(digest)),
        }
    }

    /// Resume the load under `binding` over `base`, the load's own facts where `None`.
    fn resume(
        &self,
        base: Option<&'d LoadFacts<'d>>,
        binding: Binding<'d>,
        everything: bool,
    ) -> Result<&'d LoadFacts<'d>, String> {
        (self.running)
            .resume::<Koan>(self.types, base, binding, everything, self.bump)
            .map_err(|error| error.display(self.symbols(), self.types).to_string())
    }

    /// The type `name` declares at the top level, closed.
    fn declared(&self, name: &str) -> KType {
        let root = self.root();
        let Static::Closed(typed) = self
            .load()
            .declared_type(root, slot(self.symbols(), root, name))
        else {
            panic!("`{name}` declares closed");
        };
        typed
    }
}

/// Load `source` and hand `check` the door its resumptions go through.
fn resumed<R>(source: &str, check: impl for<'d, 'graph> FnOnce(&Door<'d, 'graph>) -> R) -> R {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, SpecBounds::default(), output())
            .unwrap_or_else(|error| panic!("the program loads: {error}"));
    substrate.with(|running| {
        let bump = Bump::new();
        let types = reading(running.program().tiers(), &bump);
        check(&Door {
            running,
            types: &types,
            bump: &bump,
        })
    })
}

/// Load and run `source`, read each top-level module `modules` names back off the run, and hand
/// `check` the door beside them.
fn resumed_after_run<R>(
    source: &str,
    modules: &[&str],
    check: impl for<'d, 'graph> FnOnce(&Door<'d, 'graph>, &[(KType, ContentDigest)]) -> R,
) -> R {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, SpecBounds::default(), output())
            .unwrap_or_else(|error| panic!("the program loads: {error}"));
    substrate.with(|running| {
        running.run().expect("the program runs");
        let back: Vec<_> = (modules.iter())
            .map(|name| module_back(running, name))
            .collect();
        let bump = Bump::new();
        let types = reading(running.program().tiers(), &bump);
        let door = Door {
            running,
            types: &types,
            bump: &bump,
        };
        check(&door, &back)
    })
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
fn quantified<'graph>(
    facts: &LoadFacts<'graph>,
    root: &'graph BodyShape<'graph>,
) -> &'graph BodyShape<'graph> {
    let mut found = (root.nested_shapes().iter()).filter(|(_, nested)| {
        nested.kind() == ShapeKind::Callable && !facts.group_levels(nested).is_empty()
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

/// The narrowing of the use `LET name = <use>` in `shape`, as `facts` holds it.
fn narrowing(
    symbols: &SymbolInterner,
    facts: &LoadFacts<'_>,
    shape: &BodyShape<'_>,
    name: &str,
) -> String {
    let Some(ExpressionPart::Expression(node)) = shape.rhs(slot(symbols, shape, name)) else {
        panic!("`{name}` is bound to a node");
    };
    rendered(
        facts
            .use_statics(shape, Site::of_node(node.reference()))
            .narrowing,
    )
}

/// The upper end of the binder `name` of `shape`, as `facts` holds it, rendered.
fn upper(door: &Door<'_, '_>, facts: &LoadFacts<'_>, shape: &BodyShape<'_>, name: &str) -> String {
    let at = slot(door.symbols(), shape, name);
    let Some(DeclaredType::Type(typed)) = facts.binder_type(shape, at) else {
        panic!("`{name}` is typed, and no scheme");
    };
    display_name(typed.upper, door.types, door.symbols()).to_string()
}

/// Whether the type binder `name` of `shape` declares closed, as `facts` holds it.
fn declares_closed(
    door: &Door<'_, '_>,
    facts: &LoadFacts<'_>,
    shape: &BodyShape<'_>,
    name: &str,
) -> bool {
    let at = slot(door.symbols(), shape, name);
    matches!(facts.declared_type(shape, at), Static::Closed(_))
}

#[test]
fn binding_a_for_all_name_selects_the_overload_at_its_type() {
    let source =
        format!("{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET p = (PICK x)\n  p\n)");
    let (before, after, typed) = resumed(&source, |door| {
        let symbols = door.symbols();
        let f = quantified(door.load(), door.root());
        let before = narrowing(symbols, door.load(), f, "p");
        let binding = door.binding(&[(unknown(symbols, f, "Elt"), KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let after = narrowing(symbols, resumed, f, "p");
        (before, after, upper(door, resumed, f, "p"))
    });
    assert_eq!(before, "full");
    assert_eq!(after, "selected");
    assert_eq!(typed, "Str", "the `Number` overload returns `Str`");
}

#[test]
fn binding_a_module_parameter_closes_its_paths() {
    let source = format!(
        "{COUNTER}{PICK}LET Pinned = :(Counter WITH {{Carrier = Number}})\n\
         LET step = (FN :{{m :Counter}} -> Any = #(\n  LET p = (PICK m.zero)\n  p\n))\n"
    );
    let (before, after) = resumed(&source, |door| {
        let symbols = door.symbols();
        let step = born(symbols, door.root(), "step");
        let before = narrowing(symbols, door.load(), step, "p");
        let binding = door.binding(&[(unknown(symbols, step, "m"), door.declared("Pinned"))]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        (before, narrowing(symbols, resumed, step, "p"))
    });
    assert_eq!(before, "full");
    assert_eq!(after, "selected");
}

/// A resumption that binds a path's root module beside a `FOR ALL` name reads the path as a path,
/// not as a projection out of the type the root is bound to.
#[test]
fn binding_a_module_binder_beside_a_name_keeps_its_paths() {
    let source = format!(
        "{COUNTER}LET Pinned = :(Counter WITH {{Carrier = Number}})\n\
         EXPR FOR ALL #[Elt] #(WRAP x :Elt AND m :Counter) -> Any = #(\n  \
         LET s :{{a :Elt b :(m.Carrier)}} = {{a = x, b = m.zero}}\n  x\n)"
    );
    let resumed = resumed(&source, |door| {
        let symbols = door.symbols();
        let f = quantified(door.load(), door.root());
        let binding = door.binding(&[
            (unknown(symbols, f, "Elt"), KType::NUMBER),
            (unknown(symbols, f, "m"), door.declared("Pinned")),
        ]);
        door.resume(None, binding, false).map(|_| ())
    });
    assert_eq!(resumed, Ok(()));
}

/// A type binder that reads a `FOR ALL` name before a path over a module parameter the binding
/// leaves unbound declares unknown at the load, and stays unknown under a binding of the name: its
/// path is a class.
#[test]
fn binding_a_name_a_type_binder_reads_before_a_path_resumes() {
    let source = format!(
        "{COUNTER}EXPR FOR ALL #[Elt] #(WRAP x :Elt AND m :Counter) -> Any = #(\n  \
         LET Both = :{{a :Elt b :(m.Carrier)}}\n  x\n)"
    );
    let (before, after) = resumed(&source, |door| {
        let f = quantified(door.load(), door.root());
        let before = declares_closed(door, door.load(), f, "Both");
        let binding = door.binding(&[(unknown(door.symbols(), f, "Elt"), KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        (before, declares_closed(door, resumed, f, "Both"))
    });
    assert!(!before, "the load leaves the binder unknown");
    assert!(!after, "the binder stays unknown over the unbound path");
}

/// A type expression the load leaves unknown at a meet over a `FOR ALL` name, before a path: a
/// resumption binding the name finds the path's entry.
#[test]
fn binding_a_name_a_type_reads_before_a_path_finds_its_entry() {
    let source = format!(
        "{COUNTER}EXPR FOR ALL #[Elt] #(WRAP x :Elt AND m :Counter) -> Any = #(\n  \
         LET s :{{a :(Elt & Number) b :(m.Carrier)}} = {{a = x, b = m.zero}}\n  x\n)"
    );
    let resumed = resumed(&source, |door| {
        let f = quantified(door.load(), door.root());
        let binding = door.binding(&[(unknown(door.symbols(), f, "Elt"), KType::NUMBER)]);
        door.resume(None, binding, false).map(|_| ())
    });
    assert_eq!(resumed, Ok(()));
}

/// A binding re-runs exactly the units whose frontier holds a name it binds: `WRAP`'s, each of
/// which reads `x` and so waits on every name its entry holds, `Key` among them, and none of the
/// program's own.
#[test]
fn binding_a_name_no_fact_reads_re_runs_only_its_own() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt Key] #(WRAP x :Elt) -> Any = #(\n  LET p = (PICK x)\n  p\n)"
    );
    let (reran, waiting, outside) = resumed(&source, |door| {
        let f = quantified(door.load(), door.root());
        let key = unknown(door.symbols(), f, "Key");
        let binding = door.binding(&[(key, KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let reran: Vec<_> = (resumed.own_units().iter())
            .map(|(unit, _)| *unit)
            .collect();
        let waiting: Vec<_> = (door.load().own_units().iter())
            .filter(|(_, (_, frontier))| frontier.unknowns().contains(&key))
            .map(|(unit, _)| *unit)
            .collect();
        let outside = reran.iter().any(|unit| unit.segment != f.segment());
        (reran, waiting, outside)
    });
    assert_eq!(reran, waiting, "exactly the units waiting on `Key` re-run");
    assert!(!outside, "no unit outside `WRAP`'s body waits on `Key`");
}

#[test]
fn an_inner_callable_over_an_outer_name_settles_under_its_binding() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
         LET g = (FN :{{y :Number}} -> Any = #(\n    LET p = (PICK x)\n    p\n  ))\n  g\n)"
    );
    let (before, after) = resumed(&source, |door| {
        let symbols = door.symbols();
        let f = quantified(door.load(), door.root());
        let g = born(symbols, f, "g");
        let before = narrowing(symbols, door.load(), g, "p");
        let binding = door.binding(&[(unknown(symbols, f, "Elt"), KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        (before, narrowing(symbols, resumed, g, "p"))
    });
    assert_eq!(before, "full");
    assert_eq!(after, "selected");
}

#[test]
fn a_binding_no_overload_admits_refuses_as_the_load_would() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET y = (x + 1)\n  y\n)";
    let refused = resumed(source, |door| {
        let f = quantified(door.load(), door.root());
        let binding = door.binding(&[(unknown(door.symbols(), f, "Elt"), KType::STR)]);
        door.resume(None, binding, false).map(|_| ())
    });
    let refused = refused.expect_err("no overload admits `Str`");
    assert!(
        refused.contains("no overload of `_ + _` admits (Str, Number)"),
        "{refused}"
    );
}

#[test]
fn a_type_binder_over_a_bound_name_declares_closed() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
                  NEWTYPE Box = :{v :Elt}\n  (Box {v = x})\n)";
    let (before, after) = resumed(source, |door| {
        let f = quantified(door.load(), door.root());
        let before = declares_closed(door, door.load(), f, "Box");
        let binding = door.binding(&[(unknown(door.symbols(), f, "Elt"), KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        (before, declares_closed(door, resumed, f, "Box"))
    });
    assert!(!before, "the load leaves the binder unknown");
    assert!(after, "the resumption declares it closed");
}

#[test]
fn re_running_every_fact_settles_what_skipping_does() {
    let source =
        format!("{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET p = (PICK x)\n  p\n)");
    let (skipping, everything) = resumed(&source, |door| {
        let symbols = door.symbols();
        let f = quantified(door.load(), door.root());
        let binding = door.binding(&[(unknown(symbols, f, "Elt"), KType::NUMBER)]);
        let render = |everything| {
            let resumed =
                (door.resume(None, binding, everything)).expect("the resumption refuses nothing");
            (
                narrowing(symbols, resumed, f, "p"),
                upper(door, resumed, f, "p"),
            )
        };
        (render(false), render(true))
    });
    assert_eq!(skipping, everything);
}

/// The settled sites of `shape`'s statement `index`, as `facts` holds them.
fn settled_in(facts: &LoadFacts<'_>, shape: &BodyShape<'_>, index: usize) -> usize {
    let mut sites = Vec::new();
    crate::scope::each_part(&shape.body()[index], |part| sites.push(Site::of(part)));
    sites.push(Site::of_node(&shape.body()[index]));
    (sites.into_iter())
        .filter(|site| facts.settled(shape, *site))
        .count()
}

#[test]
fn a_check_a_binding_no_longer_settles_is_dropped() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET s :Elt = x\n  s\n)";
    let (load, skipping, everything) = resumed(source, |door| {
        let f = quantified(door.load(), door.root());
        let binding = door.binding(&[(unknown(door.symbols(), f, "Elt"), KType::NEVER)]);
        let settled = |everything| {
            let resumed =
                (door.resume(None, binding, everything)).expect("the resumption refuses nothing");
            settled_in(resumed, f, 0)
        };
        (settled_in(door.load(), f, 0), settled(false), settled(true))
    });
    assert_eq!(load, 1);
    assert_eq!((skipping, everything), (0, 0));
}

#[test]
fn a_binding_that_starves_a_refused_check_vacates_it() {
    let source = "EXPR FOR ALL #[Elt Key] #(WRAP x :Elt AND k :Key) -> Any = #(\n  \
                  LET s0 = (x :! Key)\n  x\n)";
    let (half, whole) = resumed(source, |door| {
        let symbols = door.symbols();
        let f = quantified(door.load(), door.root());
        let (elt, key) = (unknown(symbols, f, "Elt"), unknown(symbols, f, "Key"));
        let half = door.resume(None, door.binding(&[(key, KType::NEVER)]), false);
        let both = door.binding(&[(elt, KType::NEVER), (key, KType::NEVER)]);
        let whole = (door.resume(None, both, false)).expect("the resumption refuses nothing");
        (half.map(|_| ()), upper(door, whole, f, "s0"))
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
    let (first, second) = resumed(&source, |door| {
        let symbols = door.symbols();
        let f = quantified(door.load(), door.root());
        let (elt, key) = (unknown(symbols, f, "Elt"), unknown(symbols, f, "Key"));
        let once = door.binding(&[(elt, KType::NUMBER)]);
        let first = (door.resume(None, once, false)).expect("the resumption refuses nothing");
        let both = door.binding(&[(elt, KType::NUMBER), (key, KType::STR)]);
        let second =
            (door.resume(Some(first), both, false)).expect("the resumption refuses nothing");
        let render = |facts| {
            (
                narrowing(symbols, facts, f, "p"),
                narrowing(symbols, facts, f, "q"),
            )
        };
        (render(first), render(second))
    });
    assert_eq!(first, ("selected".to_string(), "full".to_string()));
    assert_eq!(second, ("selected".to_string(), "selected".to_string()));
}

/// A resumption interns what its binding closes through the chain it is handed, never into the
/// frozen root: a record type over the bound name resolves through that chain alone.
#[test]
fn a_resumption_interns_past_the_frozen_root() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
                  LET p = x\n  LET Pair = :{a :Elt}\n  p\n)";
    let (bound_upper, adopted, root_unchanged) = resumed(source, |door| {
        let tiers = door.running.program().tiers();
        let before = tiers.frozen.len();
        let bound = door.types.list(door.types.list(KType::BOOL));
        assert!(
            !door.types.in_root(bound),
            "the bound type is the chain's own"
        );
        let f = quantified(door.load(), door.root());
        let binding = door.binding(&[(unknown(door.symbols(), f, "Elt"), bound)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let p = slot(door.symbols(), f, "p");
        let bound_upper = matches!(
            resumed.binder_type(f, p),
            Some(DeclaredType::Type(typed)) if typed.upper == bound.into()
        );
        let pair = slot(door.symbols(), f, "Pair");
        let adopted = matches!(
            resumed.declared_type(f, pair),
            Static::Closed(record) if !door.types.in_root(record) && door.types.resolves(record)
        );
        (bound_upper, adopted, tiers.frozen.len() == before)
    });
    assert!(bound_upper, "`p` is the bound type");
    assert!(adopted, "`Pair` closes at a record the root lacks");
    assert!(root_unchanged, "the root holds what it held");
}

/// The overlap check is a fact of the registration's unit: a registration rigid over a `FOR ALL`
/// name is checked once a binding closes it.
#[test]
fn a_binding_that_closes_a_registration_checks_its_overlap() {
    let source = "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  OP #(+) OVER Elt = #(0)\n  x\n)";
    // The load leaves the registration rigid over `Elt`, so it loads.
    let refused = resumed(source, |door| {
        let f = quantified(door.load(), door.root());
        let elt = unknown(door.symbols(), f, "Elt");
        door.resume(None, door.binding(&[(elt, KType::NUMBER)]), false)
            .map(|_| ())
    });
    let refused = refused.expect_err("`Number` overlaps the builtin");
    assert!(
        refused.contains("this overload of `_ + _` takes operands the builtin"),
        "{refused}"
    );
}

/// A binding that closes a cycle the load did not see unfolds no call on it: the resumption runs
/// its own components over the bodies it re-runs (F3).
#[test]
fn a_binding_that_closes_a_cycle_unfolds_neither_call() {
    let source = "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  EXPR #(LOOP y :Number) -> Any = #(f {z = y})\n  \
                  EXPR #(LOOP y :Str) -> Any = #(y)\n  \
                  LET f = (FN :{z :Elt} -> Any = #(LOOP z))\n  f\n)";
    let (load, resumed) = resumed(source, |door| {
        let f = quantified(door.load(), door.root());
        let body = born(door.symbols(), f, "f");
        let recursive = |facts: &LoadFacts<'_>| {
            let recursive = |shape: &BodyShape<'_>| {
                (facts.closing(shape.segment())).is_some_and(|(closing, _)| closing.recursive)
            };
            let callables = (f.nested_shapes().iter())
                .filter(|(_, nested)| nested.kind() == ShapeKind::Callable)
                .filter(|(_, nested)| recursive(nested))
                .count();
            (recursive(body), callables)
        };
        let binding = door.binding(&[(unknown(door.symbols(), f, "Elt"), KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        (recursive(door.load()), recursive(resumed))
    });
    assert_eq!(
        load,
        (false, 0),
        "the load keeps no call into `f`, so nothing is recursive"
    );
    assert_eq!(
        resumed,
        (true, 2),
        "`f` and the `Number` overload of `LOOP` form a cycle; the `Str` one stays out of it"
    );
}

/// `SUBJECT`'s body: the last callable the top level births.
fn subject<'graph>(root: &'graph BodyShape<'graph>) -> &'graph BodyShape<'graph> {
    (root.nested_shapes().iter().rev())
        .find(|(_, nested)| nested.kind() == ShapeKind::Callable)
        .map(|(_, nested)| *nested)
        .expect("the program registers `SUBJECT`")
}

/// The site of the node `LET name = <node>` binds in `shape`.
fn node_of(symbols: &SymbolInterner, shape: &BodyShape<'_>, name: &str) -> Site {
    let Some(ExpressionPart::Expression(node)) = shape.rhs(slot(symbols, shape, name)) else {
        panic!("`{name}` is bound to a node");
    };
    Site::of_node(node.reference())
}

/// A module unknown binds to a closed content tree, so a view over a bound module parameter is
/// over a closed tree: it mints nothing at run.
#[test]
fn a_view_over_a_bound_module_is_settled() {
    let source = format!(
        "{COUNTER}MODULE bound = ((LET Carrier = Number) (LET zero = 0) \
         (LET succ = (FN :{{x :Number}} -> Number = #(x))))\n\
         EXPR #(SUBJECT m :Counter) -> Any = #(\n  LET v = (m :| Counter)\n  v\n)\n"
    );
    let (load, resumed) = resumed_after_run(&source, &["bound"], |door, back| {
        let body = subject(door.root());
        let site = node_of(door.symbols(), body, "v");
        let m = unknown(door.symbols(), body, "m");
        let binding = door.bound(&[(m, door.module(back[0]))]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        (door.load().mint_at(body, site), resumed.mint_at(body, site))
    });
    assert_eq!(load, Some(Mint::Invariant), "the load sees an open tree");
    assert_eq!(resumed, Some(Mint::Closed), "the bound tree is closed");
}

/// The upper end of the binder `name` of `SUBJECT`'s body, as `facts` holds it.
fn subject_upper(door: &Door<'_, '_>, facts: &LoadFacts<'_>, name: &str) -> String {
    upper(door, facts, subject(door.root()), name)
}

/// `Picks`, a signature over a `Carrier` whose `PICK` head a `USING` surfaces.
const PICKS: &str = "SIG Picks FOR ALL #[Carrier] = \
                     #[(VAL zero :Carrier) (EXPR #(PICK x :Carrier) -> Str)]\n";

/// A surfaced head waits on its ascription: rigid over `Elt` at the load, which leaves the head
/// unknown, and closed under `Elt := Number`, which types it as a restart with `Elt` declared does.
#[test]
fn a_surfaced_head_waits_on_its_ascription() {
    let body = "#(SUBJECT x :Elt AND m :Picks) -> Any = #(\n  \
                LET s = (USING (m :! (Picks WITH {Carrier = Elt})) SCOPE (PICK zero))\n  s\n)\n";
    let source = format!("{PICKS}EXPR FOR ALL #[Elt] {body}");
    let restart = format!("{PICKS}LET Elt = Number\nEXPR {body}");
    let (load, under) = resumed(&source, |door| {
        let elt = unknown(door.symbols(), subject(door.root()), "Elt");
        let binding = door.binding(&[(elt, KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        (
            subject_upper(door, door.load(), "s"),
            subject_upper(door, resumed, "s"),
        )
    });
    let restarted = resumed(&restart, |door| subject_upper(door, door.load(), "s"));
    assert_eq!(load, "Any", "the load leaves the head unknown");
    assert_eq!(under, restarted);
    assert_eq!(under, "Str", "the head's return");
}
