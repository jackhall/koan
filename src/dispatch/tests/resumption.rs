//! Resuming a load under a binding: the units whose frontier the binding meets re-run, each bound
//! name read as the closed type it binds, and every other unit's record stands as the load left it.
//! Each resumption goes through the test door ([`Running::resume`]), after the types' root freezes.

use crate::memory::{Bump, BumpVec, resident};
use crate::parse::ExpressionPart;
use crate::program::{CellSubstrate, Running};
use crate::scope::{
    Address, Binding, BodyClass, BodyShape, Bound, LoadFacts, Mint, ShapeKind, Site, Slot, Static,
    Unknown,
};
use crate::symbols::{BinderSymbol, SymbolInterner};
use crate::type_lattice::{DeclaredType, KType, display_name};
use crate::values::tests::reading;
use crate::values::{Chain, SpecBounds};

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

    /// What a module unknown binds to: the top-level module binder `name`, its exact type over the
    /// closed content tree the load read for it.
    fn module(&self, name: &str) -> Bound<'d> {
        let (root, load) = (self.root(), self.load());
        let at = slot(self.symbols(), root, name);
        let Some(DeclaredType::Type(typed)) = load.binder_type(root, at) else {
            panic!("`{name}` is typed");
        };
        let tree = load
            .content(root, at)
            .expect("the load reads the module's tree");
        assert!(
            typed.is_exact() && tree.closed(),
            "`{name}` is a closed module"
        );
        let carried = (self.types.concrete(typed.upper)).expect("an exact module type is closed");
        Bound::Module {
            carried,
            tree: resident(self.bump, tree),
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

/// A unit a resumption absorbs keeps what its facts gave the body's class: a porous exit a unit
/// the binding does not wake set at the load leaves the body porous in the resumption.
#[test]
fn an_absorbed_unit_keeps_the_body_s_porous_exit() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
         LET e = (EVAL #(1) -> Any)\n  LET p = (PICK x)\n  p\n)"
    );
    let (loaded, resumed) = resumed(&source, |door| {
        let f = quantified(door.load(), door.root());
        let binding = door.binding(&[(unknown(door.symbols(), f, "Elt"), KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let porous =
            |facts: &LoadFacts<'_>| matches!(facts.class(f), Some(BodyClass::Porous { .. }));
        assert!(
            (resumed.own_units().iter()).all(|(unit, _)| unit.unit.run_index() != Some(0)),
            "the `EVAL`'s unit is absorbed, not re-run"
        );
        (porous(door.load()), porous(resumed))
    });
    assert!(loaded, "the `EVAL` leaves `WRAP` porous at the load");
    assert!(resumed, "and in the resumption");
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
    let (load, resumed) = resumed(&source, |door| {
        let body = subject(door.root());
        let site = node_of(door.symbols(), body, "v");
        let m = unknown(door.symbols(), body, "m");
        let binding = door.bound(&[(m, door.module("bound"))]);
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

/// The laws over drawn [`Resumable`] programs, each fact rendered by names — never by an index or
/// a source position — so a resumption and a restart render alike where they agree.
mod laws {
    use proptest::prelude::*;

    use crate::memory::Bump;
    use crate::parse::ExpressionPart;
    use crate::program::tests::stores::framed;
    use crate::program::{CellSubstrate, LoadError};
    use crate::scope::{
        Address, BodyClass, BodyShape, Bound as Bind, CandidateList, InstanceRead, LoadFacts,
        Narrowing, ShapeError, ShapeKind, Site, Slot, Static, StaticSolution, Target, Tree,
        Unknown, each_part, source_of,
    };
    use crate::symbols::{BinderSymbol, SymbolInterner};
    use crate::type_lattice::{
        DeclaredType, Interval, KType, Parametric, Side, Variable, Verdict, display_name, fits,
        read_through,
    };
    use crate::values::tests::reading;
    use crate::values::{Chain, SpecBounds};

    use super::super::generate::{Bound, Resumable, resumable};
    use super::super::{Koan, output};
    use super::{Door, resumed, subject};

    /// The slot `name` of `shape` binds, a registration's aside.
    fn slot(symbols: &SymbolInterner, shape: &BodyShape<'_>, name: &str) -> Option<Slot> {
        (0..shape.slots()).map(|at| Slot(at as u32)).find(|at| {
            let named = shape.slot_name(*at);
            !matches!(named, BinderSymbol::Registration(_))
                && symbols.display(named.symbol()).to_string() == name
        })
    }

    /// The binding `bound` gives, read off the load: each unknown at its slot of `SUBJECT`, each
    /// type off the alias the program declares it under, each module off the module it declares.
    fn binding<'d>(door: &Door<'d, '_>, bound: &Bound) -> crate::scope::Binding<'d> {
        bind(door, bound, false)
    }

    /// [`binding`], each module bound to its content tree's digest alone, as a frame binds the
    /// module its slot holds where `by_digest`.
    fn bind<'d>(door: &Door<'d, '_>, bound: &Bound, by_digest: bool) -> crate::scope::Binding<'d> {
        let body = subject(door.root());
        let mut staged = Vec::new();
        for (name, _) in bound.entries() {
            let at = slot(door.symbols(), body, name).expect("the unknown is `SUBJECT`'s");
            let unknown = Unknown {
                segment: body.segment(),
                at: Address::Slot(at),
            };
            let declared = Resumable::declared(name);
            let given = match (
                name,
                matches!(name, "m" | "n").then(|| door.module(&declared)),
            ) {
                (_, Some(Bind::Module { carried, tree })) if by_digest => Bind::Module {
                    carried,
                    tree: crate::memory::resident(
                        door.bump,
                        Tree::Digest(tree.digest().expect("a closed tree")),
                    ),
                },
                (_, Some(module)) => module,
                (_, None) => Bind::Type(door.declared(&declared)),
            };
            staged.push((unknown, given));
        }
        door.bound(&staged)
    }

    fn interval(
        typed: Interval,
        types: &Chain<'_, '_, '_, '_>,
        symbols: &SymbolInterner,
    ) -> String {
        format!(
            "[{}, {}]",
            display_name(typed.lower, types, symbols),
            display_name(typed.upper, types, symbols)
        )
    }

    /// A load-time type, by its names.
    fn statically<C, R>(
        typed: Static<'_, C, R>,
        closed: impl Fn(C) -> String,
        rigid: impl Fn(R) -> String,
    ) -> String {
        match typed {
            Static::Unknown => "unknown".to_string(),
            Static::Closed(value) => format!("closed {}", closed(value)),
            Static::Rigid { value, .. } => format!("rigid {}", rigid(value)),
        }
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

    /// What the load fixed of a facts' reader: the chain its types render through.
    struct Reader<'read, 'facts, 'tier, 'pair, 'reg, 'door> {
        facts: &'read LoadFacts<'facts>,
        types: &'read Chain<'tier, 'pair, 'reg, 'door>,
        symbols: &'read SymbolInterner,
        /// The names a binding binds: a restart drops them from the shape or binds them anew, so
        /// what the load fixed for each binder itself is no fact a restart has.
        bound: &'read [&'read str],
    }

    impl Reader<'_, '_, '_, '_, '_, '_> {
        fn named(&self, typed: impl Into<DeclaredType<Parametric>>) -> String {
            display_name(typed, self.types, self.symbols).to_string()
        }

        fn solution(&self, solution: StaticSolution<'_>) -> String {
            match solution {
                StaticSolution::Closed(points) => {
                    let points: Vec<String> =
                        points.iter().map(|point| self.named(*point)).collect();
                    format!("closed {points:?}")
                }
                StaticSolution::Rigid { value, .. } => {
                    let points: Vec<String> =
                        value.iter().map(|point| self.named(*point)).collect();
                    format!("rigid {points:?}")
                }
            }
        }

        /// Every fact of `shape` and the shapes nested in it, rendered by names under `path`.
        fn render(&self, shape: &BodyShape<'_>, path: &str, out: &mut Vec<String>) {
            let (facts, symbols) = (self.facts, self.symbols);
            // Binders by name: a restart numbers its slots apart.
            let mut binders = Vec::new();
            for at in (0..shape.slots()).map(|at| Slot(at as u32)) {
                let named = shape.slot_name(at);
                let spelled = symbols.display(named.symbol()).to_string();
                if path.is_empty() && self.bound.contains(&spelled.as_str()) {
                    continue;
                }
                let name = format!("{path} {spelled}");
                match named {
                    BinderSymbol::Value(_) => {
                        let typed = match facts.binder_type(shape, at) {
                            Some(DeclaredType::Type(typed)) => interval(typed, self.types, symbols),
                            Some(DeclaredType::Scheme(scheme)) => self.named(scheme),
                            None => "untyped".to_string(),
                        };
                        binders.push(format!("{name} = {typed}"));
                    }
                    BinderSymbol::Type(_) => {
                        let typed = facts.declared_type(shape, at);
                        binders.push(format!(
                            "{name} declares {}",
                            statically(
                                typed,
                                |closed| self.named(closed),
                                |rigid| self.named(rigid)
                            )
                        ));
                    }
                    // A code shape's hole or mark is a capture, and holds no fact of its own.
                    BinderSymbol::Key(_) => {}
                    BinderSymbol::Registration(_) => {
                        let typed = facts.registered_type(shape, at);
                        binders.push(format!(
                            "{name} registers {}",
                            statically(
                                typed,
                                |closed| self.named(closed.shape),
                                |rigid| self.named(rigid.shape)
                            )
                        ));
                    }
                }
                if let Some(tree) = facts.content(shape, at) {
                    binders.push(format!("{name} holds a tree closed {}", tree.closed()));
                }
            }
            binders.sort();
            out.extend(binders);
            // `SUBJECT`'s own type is a fact of the unit birthing it.
            if shape.kind() == ShapeKind::Callable && !path.is_empty() {
                let typed = facts.callable_type(shape);
                out.push(format!(
                    "{path} callable {}",
                    statically(
                        typed,
                        |closed| self.named(closed.ktype),
                        |rigid| self.named(rigid.ktype)
                    )
                ));
            }
            let mut expressions: Vec<_> = shape.type_expressions().iter().collect();
            expressions.sort_by_key(|expression| expression.order);
            for (index, expression) in expressions.into_iter().enumerate() {
                let typed = facts.typed_expression(shape, expression.site);
                out.push(format!(
                    "{path} type {index} = {}",
                    statically(
                        typed,
                        |closed| self.named(closed),
                        |rigid| self.named(rigid)
                    )
                ));
            }
            // A statement binding a bound name is the restart's own, or rebinds it there.
            let skipped: Vec<usize> = (0..shape.slots())
                .map(|at| Slot(at as u32))
                .filter(|at| {
                    path.is_empty()
                        && self.bound.contains(
                            &symbols
                                .display(shape.slot_name(*at).symbol())
                                .to_string()
                                .as_str(),
                        )
                })
                .filter_map(|at| shape.position_of(at).statement_index())
                .collect();
            let statements =
                (shape.body().iter().enumerate()).filter(|(index, _)| !skipped.contains(index));
            for (written, (index, statement)) in statements.enumerate() {
                let typed = facts.statement_type(shape, index);
                let typed = typed.map_or("untyped".to_string(), |typed| {
                    interval(typed, self.types, symbols)
                });
                out.push(format!("{path} #{written} = {typed}"));
                let mut sites = Vec::new();
                each_part(statement, |part| sites.push(Site::of(part)));
                let mut at = 0;
                for site in sites.iter() {
                    let Some(typed) = facts.value_type(shape, *site) else {
                        continue;
                    };
                    let part = format!("{path} #{written}.{at}");
                    at += 1;
                    out.push(format!("{part} = {}", interval(typed, self.types, symbols)));
                    match facts.instance_at(shape, *site) {
                        Some(InstanceRead::Solved(solution)) => {
                            out.push(format!("{part} instance {}", self.solution(solution)));
                        }
                        Some(InstanceRead::AsIs) => out.push(format!("{part} instance as is")),
                        None => {}
                    }
                    let named = facts.named_contributions(shape, *site);
                    if !named.is_empty() {
                        let named: Vec<String> = (named.iter())
                            .map(|typed| {
                                statically(
                                    *typed,
                                    |closed| self.named(closed),
                                    |rigid| self.named(rigid),
                                )
                            })
                            .collect();
                        out.push(format!("{part} by name {named:?}"));
                    }
                    if let Some(tree) = facts.view_tree(shape, *site) {
                        out.push(format!("{part} views a tree closed {}", tree.closed()));
                    }
                }
                let mut uses = vec![Site::of_node(statement)];
                each_part(statement, |part| {
                    if let ExpressionPart::Expression(node) = part {
                        uses.push(Site::of_node(node.reference()));
                    }
                });
                for (at, site) in uses.into_iter().enumerate() {
                    let Some((_, list)) = shape.candidate_lists().get(site) else {
                        continue;
                    };
                    let fixed = facts.use_statics(shape, site);
                    let contributions: Vec<String> = (fixed.contributions.iter())
                        .map(|typed| {
                            statically(
                                *typed,
                                |closed| self.named(closed),
                                |rigid| self.named(rigid),
                            )
                        })
                        .collect();
                    out.push(format!(
                        "{path} #{written} use {at}: {} from {contributions:?}",
                        narrowed(fixed.narrowing, list)
                    ));
                    if let Some(points) = facts.selection(shape, site) {
                        let points: Vec<String> = (points.iter())
                            .map(|point| {
                                point.map_or("open".to_string(), |point| self.named(point))
                            })
                            .collect();
                        out.push(format!("{path} #{written} use {at} solves {points:?}"));
                    }
                }
                let settled = (sites.iter())
                    .filter(|site| facts.settled(shape, **site))
                    .count();
                out.push(format!("{path} #{written} settled {settled}"));
            }
            for (index, (_, nested)) in shape.nested_shapes().iter().enumerate() {
                self.render(nested, &format!("{path}/{index}"), out);
            }
        }
    }

    /// Every fact of `SUBJECT`'s body as `facts` holds them, but of the binders `bound` names.
    fn rendered(door: &Door<'_, '_>, facts: &LoadFacts<'_>, bound: &Bound) -> Vec<String> {
        render_facts(facts, door.types, door.symbols(), &left_out(bound))
    }

    /// The names of the binders a render under `bound` leaves out.
    fn left_out(bound: &Bound) -> Vec<String> {
        (bound.entries().iter())
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// Every fact of `SUBJECT`'s body as `facts` holds them, but of the binders `leave` names.
    fn render_facts(
        facts: &LoadFacts<'_>,
        types: &Chain<'_, '_, '_, '_>,
        symbols: &SymbolInterner,
        leave: &[String],
    ) -> Vec<String> {
        let names: Vec<&str> = leave.iter().map(String::as_str).collect();
        let reader = Reader {
            facts,
            types,
            symbols,
            bound: &names,
        };
        let mut out = Vec::new();
        reader.render(subject(facts.shapes().root()), "", &mut out);
        out
    }

    /// A refusal's message, its location dropped.
    fn unlocated(message: &str) -> String {
        match message.split_once(": ") {
            Some((at, rest)) if at.starts_with('<') => rest.to_string(),
            _ => message.to_string(),
        }
    }

    /// Resume `door`'s load under each of `bindings` in turn, each extending the one before it,
    /// and `everything` re-running every unit.
    fn resume_all<'d>(
        door: &Door<'d, '_>,
        bindings: &[Bound],
        everything: bool,
    ) -> Result<&'d LoadFacts<'d>, ShapeError<'d>> {
        let mut base: Option<&'d LoadFacts<'d>> = None;
        for bound in bindings {
            let binding = binding(door, bound);
            let resumed =
                (door.running).resume::<Koan>(door.types, base, binding, everything, door.bump)?;
            base = Some(resumed);
        }
        Ok(base.expect("a resumption under some binding"))
    }

    /// What resuming `case`'s load under each of `bindings` in turn renders, or its refusal.
    fn resume_rendered(
        case: &Resumable,
        bindings: &[Bound],
        everything: bool,
    ) -> Result<Vec<String>, String> {
        resumed(&case.render(), |door| {
            match resume_all(door, bindings, everything) {
                Ok(facts) => Ok(rendered(door, facts, bindings.last().expect("a binding"))),
                Err(error) => Err(unlocated(
                    &error.display(door.symbols(), door.types).to_string(),
                )),
            }
        })
    }

    /// What a plain load of `source` renders, or its refusal, but of the binders `bound` names.
    fn loaded_rendered(source: &str, bound: &Bound) -> Result<Vec<String>, String> {
        let loaded =
            CellSubstrate::load::<Koan>(source, "<test>", 8, SpecBounds::default(), output());
        let mut substrate = match loaded {
            Ok(substrate) => substrate,
            Err(LoadError::Shape { rendered }) => return Err(unlocated(&rendered)),
            Err(error) => panic!("the restart parses: {error}\n{source}"),
        };
        substrate.with(|running| {
            let bump = Bump::new();
            let types = reading(running.program().tiers(), &bump);
            let door = Door {
                running,
                types: &types,
                bump: &bump,
            };
            Ok(rendered(&door, door.load(), bound))
        })
    }

    /// Whether every segment of `chain`, an ancestry, holds `segment`: `segment` lies on it.
    fn on_chain(
        door: &Door<'_, '_>,
        mut at: crate::scope::Segment,
        segment: crate::scope::Segment,
    ) -> bool {
        loop {
            if at == segment {
                return true;
            }
            match door.running.program().shapes().parent(at) {
                Some((parent, _)) => at = parent,
                None => return false,
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: crate::tests::case_share(1, 4),
            ..ProptestConfig::default()
        })]

        /// 1. A resumption is a restart: resuming the load under a binding renders every fact of
        /// `SUBJECT`'s body — type and value facts alike — as a load of the program with the
        /// binding written in does, refusals included.
        #[test]
        fn a_resumption_is_a_restart(case in resumable()) {
            let resumed = resume_rendered(&case, std::slice::from_ref(&case.bound), false);
            let restart = case.restart(&case.bound);
            prop_assert_eq!(resumed, loaded_rendered(&restart, &case.bound), "in\n{}\nrestarted as\n{}", case.render(), restart);
        }

        /// 2. Resumptions are confluent: reaching a binding in two steps, either half first,
        /// settles what reaching it in one does. A refusal on the way agrees with a binding that
        /// leaves its site vacuous, as the monotone law reads one typed `Never`.
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

        /// 3. Quiescence is exact: a resumption re-running every unit renders as one waking only
        /// what the binding meets, and every unit frontier's unknowns are declared on its chain.
        #[test]
        fn quiescence_is_exact(case in resumable()) {
            let skipping = resume_rendered(&case, std::slice::from_ref(&case.bound), false);
            let everything = resume_rendered(&case, std::slice::from_ref(&case.bound), true);
            prop_assert_eq!(skipping, everything, "in\n{}", case.render());
            let foreign = resumed(&case.render(), |door| {
                (door.load().own_units().iter())
                    .flat_map(|(unit, (_, frontier))| {
                        frontier.unknowns().iter().filter(|unknown| !on_chain(door, unit.segment, unknown.segment))
                            .map(move |unknown| format!("{unit:?} waits on {unknown:?}"))
                    })
                    .collect::<Vec<_>>()
            });
            prop_assert!(foreign.is_empty(), "{:?} in\n{}", foreign, case.render());
        }

        /// 4. A resumption is monotone: each fact it re-runs lies within the load's with the
        /// binding substituted, a verdict of *always* stays, a narrowing only shrinks, and a
        /// settled site stays settled. A fact typed `Never` never arrives: it is the bottom.
        #[test]
        fn a_resumption_is_monotone(case in resumable()) {
            let broken = resumed(&case.render(), |door| monotone(door, &case.bound));
            prop_assert!(broken.is_empty(), "{:?} in\n{}", broken, case.render());
        }

        /// 5. A resumption re-runs exactly the units whose frontier at the load meets its binding,
        /// each once: so it terminates.
        #[test]
        fn a_resumption_reruns_only_the_units_it_wakes(case in resumable()) {
            let compared = resumed(&case.render(), |door| {
                let binding = binding(door, &case.bound);
                let resumed = (door.running).resume::<Koan>(door.types, None, binding, false, door.bump).ok()?;
                let reran: Vec<_> = resumed.own_units().iter().map(|(unit, _)| *unit).collect();
                let woken: Vec<_> = (door.load().own_units().iter())
                    .filter(|(_, (_, frontier))| frontier.meets_binding(binding))
                    .map(|(unit, _)| *unit)
                    .collect();
                Some((reran, woken))
            });
            // A resumption that refuses re-runs nothing to compare: law 1 holds its refusal.
            if let Some((reran, woken)) = compared {
                prop_assert_eq!(reran, woken, "in\n{}", case.render());
            }
        }

        /// 6. The rigid rule commutes with binding: each type fact the load fixes rigid over names
        /// the binding binds, substituted, is the restart's closed fact.
        #[test]
        fn the_rigid_rule_commutes_with_binding(case in resumable()) {
            let substituted = resumed(&case.render(), |door| substituted(door, &case.bound));
            let restart = case.restart(&case.bound);
            // A restart that refuses fixes no closed fact to compare: law 1 holds its refusal.
            if let Ok(restarted) = loaded_rendered(&restart, &case.bound) {
                for line in substituted {
                    prop_assert!(restarted.contains(&line), "{} not in the restart\n{}\nrestarted as\n{}", line, case.render(), restart);
                }
            }
        }

        /// 8. Three stores give equal facts under one binding: a load's store binding it from the
        /// start, a layer over the load, and a frame whose slots hold what it binds — read off the
        /// top level once the program has run, each module bound by its digest, as a frame binds
        /// one — render alike, refusals included.
        #[test]
        fn three_stores_give_equal_facts(case in resumable()) {
            let unlocate = |error: ShapeError<'_>, door: &Door<'_, '_>| unlocated(&error.display(door.symbols(), door.types).to_string());
            let stores = resumed(&case.render(), |door| {
                // The load's store and a layer agree under a structural binding too.
                [true, false].map(|by_digest| {
                    let binding = bind(door, &case.bound, by_digest);
                    let reloaded = (door.running).reload::<Koan>(door.types, binding, door.bump);
                    let reloaded = reloaded.map(|facts| rendered(door, facts, &case.bound)).map_err(|error| unlocate(error, door));
                    let layered = (door.running).resume::<Koan>(door.types, None, binding, false, door.bump);
                    let layered = layered.map(|facts| rendered(door, facts, &case.bound)).map_err(|error| unlocate(error, door));
                    (reloaded, layered)
                })
            });
            for (reloaded, layered) in &stores {
                prop_assert_eq!(reloaded, layered, "in\n{}", case.render());
            }
            let [(_, layered), _] = stores;
            let mut substrate = CellSubstrate::load::<Koan>(&case.render(), "<test>", 8, SpecBounds::default(), output())
                .expect("the program loads");
            let framed = substrate.with(|running| {
                running.run().expect("the program runs");
                let (root, symbols) = (running.program().shapes().root(), running.symbols());
                let body = subject(root);
                let slots: Vec<_> = (case.bound.entries().iter())
                    .map(|(name, _)| {
                        let at = slot(symbols, body, name).expect("the unknown is `SUBJECT`'s");
                        let unknown = Unknown { segment: body.segment(), at: Address::Slot(at) };
                        (unknown, Resumable::declared(name))
                    })
                    .collect();
                framed::<Koan>(running, slots, render_facts, left_out(&case.bound))
            });
            let framed = framed.map_err(|error| unlocated(&error));
            prop_assert_eq!(&layered, &framed, "in\n{}", case.render());
        }

        /// 9. Levels and path identities depend on the shape alone: every path entry a record
        /// holds at a level the shape numbers is the path the shape spells there, at the load and
        /// under the binding, so no record changes a level. The shape's own numbering is
        /// `scope`'s `two_builds_of_one_text_agree`.
        #[test]
        fn levels_and_path_identities_depend_on_the_shape_alone(case in resumable()) {
            let broken = resumed(&case.render(), |door| {
                let shapes = door.running.program().shapes();
                let mut facts = vec![door.load()];
                // A resumption that refuses holds no records; the load's are still checked.
                if let Ok(resumed) = resume_all(door, std::slice::from_ref(&case.bound), false) {
                    facts.push(resumed);
                }
                let mut broken = Vec::new();
                for shape in shapes.bodies() {
                    let root = shapes.shape_at(shapes.chain_root(shape.segment()));
                    for (index, spelled) in shape.spelled().iter().enumerate() {
                        let (Some(id), Some(level)) = (spelled.id, shapes.path_level(shape.segment(), index)) else {
                            continue;
                        };
                        for (at, facts) in facts.iter().enumerate() {
                            if let Some(entry) = facts.path(root, level).map(|(entry, _)| entry)
                                && (entry.root != id.root || entry.chain != id.chain || entry.label != id.label || entry.level != level)
                            {
                                broken.push(format!("records {at} hold {entry:?} at the level the shape gives {id:?}"));
                            }
                        }
                    }
                }
                broken
            });
            prop_assert!(broken.is_empty(), "{:?} in\n{}", broken, case.render());
        }

        /// 7. A body sealed at the load is sealed in every restart: a binding only closes what
        /// the load left open, and a closed exit is no porous one.
        #[test]
        fn a_body_sealed_at_the_load_is_sealed_in_every_restart(case in resumable()) {
            let sealed = resumed(&case.render(), |door| classes(door, door.load()));
            let restart = case.restart(&case.bound);
            let Ok(mut substrate) = CellSubstrate::load::<Koan>(&restart, "<test>", 8, SpecBounds::default(), output()) else {
                return Ok(());
            };
            let restarted = substrate.with(|running| {
                let bump = Bump::new();
                let types = reading(running.program().tiers(), &bump);
                let door = Door { running, types: &types, bump: &bump };
                classes(&door, door.load())
            });
            // The restart's own `LET`s birth no body, so both hold one frame body at each path.
            let paths = |classes: &[(String, bool)]| classes.iter().map(|(path, _)| path.clone()).collect::<Vec<_>>();
            prop_assert_eq!(paths(&sealed), paths(&restarted), "in\n{}\nrestarted as\n{}", case.render(), restart);
            for ((path, was), (_, is)) in sealed.iter().zip(restarted.iter()) {
                if *was {
                    prop_assert!(*is, "{} is porous in the restart\n{}\nrestarted as\n{}", path, case.render(), restart);
                }
            }
        }
    }

    /// Whether each frame body of `SUBJECT`'s subtree is sealed, by path: each step a nested
    /// body's place in program order, never its site, an address.
    fn classes(door: &Door<'_, '_>, facts: &LoadFacts<'_>) -> Vec<(String, bool)> {
        fn walk(
            facts: &LoadFacts<'_>,
            shape: &BodyShape<'_>,
            path: String,
            out: &mut Vec<(String, bool)>,
        ) {
            if let Some(class) = facts.class(shape) {
                out.push((path.clone(), matches!(class, BodyClass::Sealed { .. })));
            }
            let nested =
                (0..shape.statements() as usize).flat_map(|statement| shape.nested_in(statement));
            for (index, (_, nested)) in nested.enumerate() {
                walk(facts, nested, format!("{path}/{index}"), out);
            }
        }
        let mut out = Vec::new();
        walk(facts, subject(door.root()), String::new(), &mut out);
        out
    }

    /// The levels `bound` gives a type, read off `SUBJECT`'s homes.
    fn levels(door: &Door<'_, '_>, bound: &Bound) -> Vec<(usize, KType)> {
        let body = subject(door.root());
        let binding = binding(door, bound);
        let mut levels = Vec::new();
        for (level, target) in door.load().homes(body) {
            let Target::Local(slot) = target else {
                continue;
            };
            let unknown = Unknown {
                segment: body.segment(),
                at: Address::Slot(*slot),
            };
            if let (Some(Bind::Type(typed)), BinderSymbol::Type(_)) =
                (binding.bound(unknown), body.slot_name(*slot))
            {
                levels.push((*level, typed));
            }
        }
        levels
    }

    /// `value` with each level `levels` binds read as its type, where every lexical variable it
    /// names is bound: `None` where one is not.
    fn substitute(
        door: &Door<'_, '_>,
        levels: &[(usize, KType)],
        value: Parametric,
    ) -> Option<Parametric> {
        let scratch = Bump::new();
        let mut open = false;
        let read = read_through(door.types, &scratch, value, Side::Above, &mut |variable| {
            let Variable::Lexical { level, .. } = variable else {
                return None;
            };
            match levels.iter().find(|(held, _)| *held == level) {
                Some((_, typed)) => Some(Interval::point((*typed).into())),
                None => {
                    open = true;
                    None
                }
            }
        });
        (!open).then_some(read)
    }

    /// The lines of `SUBJECT`'s render of each type fact the load fixes rigid — a type expression's
    /// type, a nested callable's type, a registration's bucket entry — read with the binding `bound`
    /// substituted, where it binds every name the fact reads: what the run solves at its use.
    fn substituted(door: &Door<'_, '_>, bound: &Bound) -> Vec<String> {
        let levels = levels(door, bound);
        let (facts, types, symbols) = (door.load(), door.types, door.symbols());
        let named = |typed: Parametric| display_name(typed, types, symbols).to_string();
        let read = |typed: DeclaredType<Parametric>| match typed {
            DeclaredType::Type(typed) => substitute(door, &levels, typed).map(named),
            // A scheme's own group is no name the binding binds.
            DeclaredType::Scheme(_) => None,
        };
        let mut out = Vec::new();
        let mut shapes = vec![(subject(door.root()), String::new())];
        while let Some((shape, path)) = shapes.pop() {
            for (index, (_, nested)) in shape.nested_shapes().iter().enumerate() {
                shapes.push((nested, format!("{path}/{index}")));
            }
            let mut expressions: Vec<_> = shape.type_expressions().iter().collect();
            expressions.sort_by_key(|expression| expression.order);
            for (index, expression) in expressions.into_iter().enumerate() {
                if let Static::Rigid { value, .. } = facts.typed_expression(shape, expression.site)
                    && let Some(read) = read(DeclaredType::Type(value))
                {
                    out.push(format!("{path} type {index} = closed {read}"));
                }
            }
            if shape.kind() == ShapeKind::Callable
                && !path.is_empty()
                && let Static::Rigid { value, .. } = facts.callable_type(shape)
                && let Some(read) = read(value.ktype)
            {
                out.push(format!("{path} callable closed {read}"));
            }
            // A type binder's declared type is closed or unknown, never rigid: none to substitute.
            for at in (0..shape.slots()).map(|at| Slot(at as u32)) {
                if let BinderSymbol::Registration(name) = shape.slot_name(at)
                    && let Static::Rigid { value, .. } = facts.registered_type(shape, at)
                    && let Some(read) = read(value.shape)
                {
                    out.push(format!(
                        "{path} {} registers closed {read}",
                        symbols.display(name.symbol())
                    ));
                }
            }
        }
        out
    }

    /// Whether the refusal met reaching `case`'s binding by `first` and then the rest lies in a
    /// statement the whole binding types `Never`, of `SUBJECT`'s body or a body nested in it: no
    /// operator run reaches it.
    fn vacated(case: &Resumable, first: &[&str]) -> bool {
        resumed(&case.render(), |door| {
            let refused =
                resume_all(door, &[case.bound.only(first), case.bound.clone()], false).err();
            let (Some(refused), Ok(whole)) = (
                refused,
                resume_all(door, std::slice::from_ref(&case.bound), false),
            ) else {
                return false;
            };
            let at = refused.at().span;
            let mut shapes = vec![subject(door.root())];
            while let Some(shape) = shapes.pop() {
                shapes.extend(shape.nested_shapes().iter().map(|(_, nested)| *nested));
                let vacuous = (shape.body().iter().enumerate()).any(|(index, statement)| {
                    let span = statement.source.span;
                    whole
                        .statement_type(shape, index)
                        .is_some_and(|typed| typed.upper == KType::NEVER.into())
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

    /// Each fact of `SUBJECT`'s body a resumption under `bound` re-runs outside the load's with
    /// the binding substituted, rendered: the binders, statements and parts whose load value names
    /// only lexical variables the binding binds, and every use and settled site.
    fn monotone(door: &Door<'_, '_>, bound: &Bound) -> Vec<String> {
        let Ok(resumed) = resume_all(door, std::slice::from_ref(bound), false) else {
            return Vec::new();
        };
        let (types, symbols) = (door.types, door.symbols());
        let levels = levels(door, bound);
        let scratch = Bump::new();
        let load = door.load();
        let mut broken = Vec::new();
        // A fact that never arrives is the bottom, within every fact.
        let never = |typed: Interval| typed.upper == KType::NEVER.into();
        let outside = |what: String, was: Interval, is: Interval| -> Option<String> {
            if never(is) {
                return None;
            }
            let (lower, upper) = (
                substitute(door, &levels, was.lower)?,
                substitute(door, &levels, was.upper)?,
            );
            let within =
                fits(types, &scratch, lower, is.lower) && fits(types, &scratch, is.upper, upper);
            (!within).then(|| {
                format!(
                    "{what}: {} outside {}",
                    interval(is, types, symbols),
                    interval(Interval { lower, upper }, types, symbols)
                )
            })
        };
        let mut shapes = vec![subject(door.root())];
        while let Some(shape) = shapes.pop() {
            shapes.extend(shape.nested_shapes().iter().map(|(_, nested)| *nested));
            for at in (0..shape.slots()).map(|at| Slot(at as u32)) {
                if let (Some(DeclaredType::Type(was)), Some(DeclaredType::Type(is))) =
                    (load.binder_type(shape, at), resumed.binder_type(shape, at))
                {
                    broken.extend(outside(format!("binder {}", at.index()), was, is));
                }
            }
            for index in 0..shape.body().len() {
                if let (Some(was), Some(is)) = (
                    load.statement_type(shape, index),
                    resumed.statement_type(shape, index),
                ) {
                    broken.extend(outside(format!("statement {index}"), was, is));
                }
            }
            // A use or a check in a statement that never completes never runs.
            let vacuous = |site: Site| {
                (shape.body().iter().enumerate()).any(|(index, statement)| {
                    resumed.statement_type(shape, index).is_some_and(never)
                        && source_of(statement, site).is_some()
                })
            };
            for statement in shape.body() {
                let mut sites = vec![Site::of_node(statement)];
                each_part(statement, |part| {
                    sites.push(Site::of(part));
                    if let ExpressionPart::Expression(node) = part {
                        sites.push(Site::of_node(node.reference()));
                    }
                });
                for site in sites {
                    if let (Some(was), Some(is)) = (
                        load.value_type(shape, site),
                        resumed.value_type(shape, site),
                    ) {
                        broken.extend(outside(format!("part {site:?}"), was, is));
                    }
                    if vacuous(site) {
                        continue;
                    }
                    if shape.candidate_lists().get(site).is_some()
                        && let Some(why) = widened(
                            load.use_statics(shape, site).narrowing,
                            resumed.use_statics(shape, site).narrowing,
                        )
                    {
                        broken.push(format!("use {site:?}: {why}"));
                    }
                    if load.settled(shape, site) && !resumed.settled(shape, site) {
                        broken.push(format!("settled {site:?} unsettled"));
                    }
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
