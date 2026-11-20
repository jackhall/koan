//! Resuming a load under a binding: the units whose frontier the binding meets re-run, each bound
//! name read as the closed type it binds, and every other unit's record stands as the load left it.
//! Each resumption goes through the test door ([`Running::resume`]), after the types' root freezes.

use crate::memory::{Bump, BumpVec, resident};
use crate::parse::ExpressionPart;
use crate::program::{CellSubstrate, Running};
use crate::scope::{
    Address, Binding, BodyClass, BodyShape, Bound, Identity, LoadFacts, Mint, ShapeKind, Site,
    Slot, Static, Unknown,
};
use crate::symbols::{BinderSymbol, SymbolInterner};
use crate::type_lattice::{DeclaredType, KType, display_name};
use crate::values::tests::reading;
use crate::values::{BuildBudget, Chain};

use super::statics::rendered;
use super::{Koan, output};

/// Two overloads of `PICK`, one at `Number` and one at `Str`.
const PICK: &str = "EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                    EXPR #(PICK x :Str) -> Number = #(1)\n";

/// `SIG Counter`, a module signature over a `Carrier` head parameter.
pub(super) const COUNTER: &str = "SIG Counter FOR ALL #[Carrier] = \
                       #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n";

/// What a test reads a resumption through: the running program, a chain over its frozen root that
/// every resumption interns through, and the bump they lay their records in.
pub(super) struct Door<'d, 'graph> {
    running: &'d Running<'graph>,
    pub(super) types: &'d Chain<'d, 'd, 'd, 'd>,
    pub(super) bump: &'d Bump,
}

impl<'d, 'graph: 'd> Door<'d, 'graph> {
    pub(super) fn symbols(&self) -> &'graph SymbolInterner {
        self.running.symbols()
    }

    pub(super) fn root(&self) -> &'graph BodyShape<'graph> {
        self.running.program().shapes().root()
    }

    pub(super) fn load(&self) -> &'graph LoadFacts<'graph> {
        self.running.program().facts()
    }

    /// `staged` laid down as a binding of types.
    pub(super) fn binding(&self, staged: &[(Unknown, KType)]) -> Binding<'d> {
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
        let Some(DeclaredType::Type(typed)) = load.binder_type(self.types, root, at) else {
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

    /// The resumption of the load under `own`: closed where `closed`, else over `over`, a witnessed
    /// layer, else witnessed afresh.
    pub(super) fn resumption(
        &self,
        own: Binding<'d>,
        over: Option<&'d crate::scope::BuildLayer<'d>>,
        closed: bool,
    ) -> Result<crate::program::resume::Resumed<'d>, String> {
        (self.running)
            .resumption::<Koan>(self.types, own, (over, closed), self.bump)
            .map_err(|error| error.display(self.symbols(), self.types).to_string())
    }

    /// Witness the load under `own`: the first walk of a witnessing resumption alone.
    pub(super) fn witness(&self, own: Binding<'d>) -> Result<&'d LoadFacts<'d>, String> {
        (self.running)
            .witness::<Koan>(self.types, own, self.bump)
            .map_err(|error| error.display(self.symbols(), self.types).to_string())
    }

    /// Resume the load under `binding` over `base`, the load's own facts where `None`.
    pub(super) fn resume(
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
    pub(super) fn declared(&self, name: &str) -> KType {
        let root = self.root();
        let Static::Closed(typed) =
            self.load()
                .declared_type(self.types, root, slot(self.symbols(), root, name))
        else {
            panic!("`{name}` declares closed");
        };
        typed
    }
}

/// Load `source` and hand `check` the door its resumptions go through.
pub(super) fn resumed<R>(
    source: &str,
    check: impl for<'d, 'graph> FnOnce(&Door<'d, 'graph>) -> R,
) -> R {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output())
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
pub(super) fn unknown(symbols: &SymbolInterner, shape: &BodyShape<'_>, name: &str) -> Unknown {
    Unknown {
        segment: shape.segment(),
        at: Address::Slot(slot(symbols, shape, name)),
    }
}

/// The one quantified callable body the top level registers.
pub(super) fn quantified<'graph>(door: &Door<'_, 'graph>) -> &'graph BodyShape<'graph> {
    let mut found = (door.root().nested_shapes().iter()).filter(|(_, nested)| {
        nested.kind() == ShapeKind::Callable
            && !door.load().group_levels(door.types, nested).is_empty()
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
    door: &Door<'_, '_>,
    facts: &LoadFacts<'_>,
    shape: &BodyShape<'_>,
    name: &str,
) -> String {
    let Some(ExpressionPart::Expression(node)) = shape.rhs(slot(door.symbols(), shape, name))
    else {
        panic!("`{name}` is bound to a node");
    };
    rendered(
        facts
            .use_statics(
                door.types,
                &crate::memory::Bump::new(),
                shape,
                shape.site_of_node(node),
            )
            .narrowing,
    )
}

/// The upper end of the binder `name` of `shape`, as `facts` holds it, rendered.
fn upper(door: &Door<'_, '_>, facts: &LoadFacts<'_>, shape: &BodyShape<'_>, name: &str) -> String {
    let at = slot(door.symbols(), shape, name);
    let Some(DeclaredType::Type(typed)) = facts.binder_type(door.types, shape, at) else {
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
    matches!(
        facts.declared_type(door.types, shape, at),
        Static::Closed(_)
    )
}

#[test]
fn binding_a_for_all_name_selects_the_overload_at_its_type() {
    let source =
        format!("{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET p = (PICK x)\n  p\n)");
    let (before, after, typed) = resumed(&source, |door| {
        let symbols = door.symbols();
        let f = quantified(door);
        let before = narrowing(door, door.load(), f, "p");
        let binding = door.binding(&[(unknown(symbols, f, "Elt"), KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let after = narrowing(door, resumed, f, "p");
        (before, after, upper(door, resumed, f, "p"))
    });
    assert_eq!(before, "kept maybe maybe");
    assert_eq!(after, "selected");
    assert_eq!(typed, "Str", "the `Number` overload returns `Str`");
}

/// How the use `LET name = <use>` in `shape` ranks its candidates, as `facts` holds it: open, by a
/// recorded table, or selected.
fn elimination(
    door: &Door<'_, '_>,
    facts: &LoadFacts<'_>,
    shape: &BodyShape<'_>,
    name: &str,
) -> &'static str {
    let Some(ExpressionPart::Expression(node)) = shape.rhs(slot(door.symbols(), shape, name))
    else {
        panic!("`{name}` is bound to a node");
    };
    let site = shape.site_of_node(node);
    match facts
        .use_statics(door.types, &crate::memory::Bump::new(), shape, site)
        .narrowing
    {
        crate::scope::Narrowing::Kept(kept) => match kept.elimination {
            crate::scope::Elimination::Open => "open",
            crate::scope::Elimination::Table(_) => "table",
        },
        crate::scope::Narrowing::Full => "open",
        crate::scope::Narrowing::Selected(_) => "selected",
    }
}

#[test]
fn a_rigid_pair_left_open_at_the_load_is_ranked_by_its_settlement() {
    // `PICK y :Elt` is rigid at the load, where ranking it beside `PICK y :Str` would read `Elt`
    // through its bound: the load leaves the pair open. Bound to `Number`, both are closed, and
    // the resumption records their table.
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt z :Any) -> Any = #(\n  \
                  EXPR #(PICK y :Elt) -> Str = #(\"e\")\n  \
                  EXPR #(PICK y :Str) -> Number = #(1)\n  \
                  LET p = (PICK z)\n  p\n)";
    let (before, after) = resumed(source, |door| {
        let symbols = door.symbols();
        let f = quantified(door);
        let before = elimination(door, door.load(), f, "p");
        let binding = door.binding(&[(unknown(symbols, f, "Elt"), KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        (before, elimination(door, resumed, f, "p"))
    });
    assert_eq!((before, after), ("open", "table"));
}

/// A rigid pair's ranking is a question per pair: the use holds nothing over `Elt`, and a second
/// binding ranking the pair alike keeps its answers and lays no unit for it.
#[test]
fn a_rigid_pair_s_ranking_is_asked_and_kept_under_a_second_binding() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt z :Any) -> Any = #(\n  \
                  EXPR #(PICK y :Elt) -> Str = #(\"e\")\n  \
                  EXPR #(PICK y :Str) -> Number = #(1)\n  \
                  LET p = (PICK z)\n  p\n)";
    let (holding, laid) = resumed(source, |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let unit = unit_of(door, f, "p");
        let holding = (waits_on(door, unit, elt).into_iter().flatten().flatten())
            .filter(|kind| kind.holds())
            .collect::<Vec<_>>();
        let under = |at| door.binding(&[(elt, at)]);
        let first = (door.resumption(under(KType::NUMBER), None, false)).expect("no refusal");
        let witnessed = first.witnessed.expect("the first resumption witnesses");
        let second = door.resumption(under(KType::BOOL), Some(witnessed), false);
        (
            holding,
            second.expect("no refusal").layer.unit_of(unit).is_some(),
        )
    });
    assert!(holding.is_empty(), "{holding:?}");
    assert!(
        !laid,
        "a binding ranking the pair alike lays no unit for `p`"
    );
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
        let before = narrowing(door, door.load(), step, "p");
        let binding = door.binding(&[(unknown(symbols, step, "m"), door.declared("Pinned"))]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        (before, narrowing(door, resumed, step, "p"))
    });
    assert_eq!(before, "kept maybe maybe");
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
        let f = quantified(door);
        let binding = door.binding(&[
            (unknown(symbols, f, "Elt"), KType::NUMBER),
            (unknown(symbols, f, "m"), door.declared("Pinned")),
        ]);
        door.resume(None, binding, false).map(|_| ())
    });
    assert_eq!(resumed, Ok(()));
}

/// A binding of a module parameter is written into the path variables of its classes: the
/// parameter's seed, exact at its own paths, is absorbed and closed by module substitution to the
/// type its retype stamps, and a read of its member is what a fresh load reads.
#[test]
fn binding_a_module_parameter_writes_it_into_its_paths() {
    let source = format!(
        "{COUNTER}MODULE ints = ((LET Carrier = Number) (LET zero = 0) \
         (LET succ = (FN :{{x :Number}} -> Number = #(x))))\n\
         LET stamped :Counter = ints\n\
         LET step = (FN :{{m :Counter}} -> Any = #(\n  LET k = m.zero\n  k\n))\n"
    );
    let (held, seed, seed_reran, read) = resumed(&source, |door| {
        let symbols = door.symbols();
        let step = born(symbols, door.root(), "step");
        let (entry, _) = (door.load().unit(unit_of(door, step, "m"))).expect("the entry ran");
        let binding = door.bound(&[(unknown(symbols, step, "m"), door.module("stamped"))]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        (
            entry.paths.len(),
            upper(door, resumed, step, "m"),
            reran(resumed, unit_of(door, step, "m")),
            upper(door, resumed, step, "k"),
        )
    });
    assert_eq!(held, 1, "the entry's record holds the seed's path");
    assert!(seed.ends_with("WITH {Carrier = Number}"), "{seed}");
    assert!(!seed_reran, "the seed names its paths alone");
    assert_eq!(read, "Number");
}

/// A path read through a binder a `LET` retypes at a signature, over a module parameter, closes by
/// module substitution: under a binding of the parameter, `n.Carrier` is the carrier the retyped
/// module shows, as a fresh load reads it, and the unit reading it re-runs nothing.
#[test]
fn a_path_through_a_retyped_binder_closes_as_the_module_it_shows() {
    let source = format!(
        "{COUNTER}MODULE ints = ((LET Carrier = Number) (LET zero = 0) \
         (LET succ = (FN :{{x :Number}} -> Number = #(x))))\n\
         LET stamped :Counter = ints\n\
         LET step = (FN :{{m :Counter}} -> Any = #(\n  LET n :Counter = m\n  \
         LET Carriers = :(n.Carrier)\n  Carriers\n))\n"
    );
    let (load, under, carriers_reran) = resumed(&source, |door| {
        let symbols = door.symbols();
        let step = born(symbols, door.root(), "step");
        let load = declares_closed(door, door.load(), step, "Carriers");
        let binding = door.bound(&[(unknown(symbols, step, "m"), door.module("stamped"))]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let at = slot(symbols, step, "Carriers");
        let under = match resumed.declared_type(door.types, step, at) {
            Static::Closed(closed) => Some(display_name(closed, door.types, symbols).to_string()),
            _ => None,
        };
        (load, under, reran(resumed, unit_of(door, step, "Carriers")))
    });
    assert!(!load, "the load reads `n.Carrier` as its class");
    assert_eq!(under.as_deref(), Some("Number"));
    assert!(!carriers_reran, "the record names `m`, and is absorbed");
}

/// A module parameter held in place is pinned at the paths through its operand, so a read of it
/// is over `m`'s own class at the load: under a binding of `m` it is what a fresh load reads, no
/// question it asks holds, and a second binding keeping every answer lays no unit for it. A hold
/// of a hold, a hold of a member chain, and a hold of a `:Module` parameter are pinned at the same
/// paths, and a member read off the hold reads the type it arrives at.
#[test]
fn a_module_held_in_place_is_pinned_at_its_operand_s_paths() {
    let prelude = format!(
        "{COUNTER}MODULE ints = ((LET Carrier = Number) (LET zero = 0) \
         (LET succ = (FN :{{x :Number}} -> Number = #(x))))\n\
         LET stamped :Counter = ints\n\
         SIG Outer = #[(VAL inner :Counter)]\n\
         MODULE outer = ((LET inner = stamped))\n"
    );
    let using = "LET s0 = (USING (m :! Counter) SCOPE (succ zero))";
    let inner_using = "LET s0 = (USING (m.inner :! Counter) SCOPE (succ zero))";
    for (param, module, body) in [
        ("m :Counter", "stamped", using),
        ("m :Counter", "stamped", "LET s0 = (m :! Counter).zero"),
        (
            "m :Counter",
            "stamped",
            "LET s0 = ((m :! Counter) :! Counter).zero",
        ),
        ("m :Outer", "outer", "LET s0 = (m.inner :! Counter).zero"),
        ("m :Outer", "outer", inner_using),
        ("m :Module", "stamped", "LET s0 = (m :! Counter).zero"),
    ] {
        let program = |param: &str, prefix: &str| {
            format!(
                "{prelude}EXPR FOR ALL #[Elt] #(SUBJECT x :Elt AND {param}) -> Any = #(\n  \
                 {prefix}{body}\n  s0\n)\n"
            )
        };
        let (load, under, holding, laid) = resumed(&program(param, ""), |door| {
            let symbols = door.symbols();
            let f = subject(door.root());
            let (elt, m) = (unknown(symbols, f, "Elt"), unknown(symbols, f, "m"));
            let unit = unit_of(door, f, "s0");
            let holding = (waits_on(door, unit, m).into_iter().flatten().flatten())
                .filter(|kind| kind.holds())
                .collect::<Vec<_>>();
            let under =
                |elt_at| door.bound(&[(elt, Bound::Type(elt_at)), (m, door.module(module))]);
            let first = (door.resumption(under(KType::NUMBER), None, false)).expect("no refusal");
            let witnessed = first.witnessed.expect("the first resumption witnesses");
            let second = door.resumption(under(KType::BOOL), Some(witnessed), false);
            let laid = second.expect("no refusal").layer.unit_of(unit).is_some();
            let resumed = (door.resume(None, under(KType::NUMBER), false)).expect("no refusal");
            let load = upper(door, door.load(), f, "s0");
            (load, upper(door, resumed, f, "s0"), holding, laid)
        });
        let restart = program("n :Any", &format!("LET m = {module}\n  "));
        let restarted = resumed(&restart, |door| {
            upper(door, door.load(), subject(door.root()), "s0")
        });
        assert_ne!(
            load, "Any",
            "{body}: the load reads the hold over `m`'s class"
        );
        assert_eq!(under, restarted, "{body}");
        assert_eq!(under, "Number", "{body}");
        assert!(holding.is_empty(), "{body}: {holding:?}");
        assert!(
            !laid,
            "{body}: a binding keeping every answer lays no unit for `s0`"
        );
    }
}

/// A value member read through a type a run binds is the member the binding's module declares,
/// read whole over a union, and unknown where the module declares none: under each binding of
/// `Elt`, the load's type closed by the binding is the resumption's, and a restart's, and no
/// question the read asks holds.
#[test]
fn a_member_read_through_a_run_bound_name_is_what_a_fresh_load_reads() {
    let prefix = "SIG Has = #[(VAL f :Number)]\nSIG Deep = #[(VAL f :Has2)]\n\
                  SIG Has2 = #[(VAL g :Str)]\n";
    let chained = "#(SUBJECT x :Elt) -> Any = #(\n  LET w = x.f\n  LET y = w.g\n  y\n)";
    for (body, declared, expected) in [
        (
            "#(SUBJECT x :Elt) -> Any = #(\n  LET y = x.f\n  y\n)",
            ":Module",
            "[Never, Any]",
        ),
        (chained, ":Deep", "[Str, Str]"),
        (
            "#(SUBJECT x :(Elt | Has)) -> Any = #(\n  LET y = x.f\n  y\n)",
            ":Never",
            "[Number, Number]",
        ),
    ] {
        let source = format!("{prefix}LET Bound = {declared}\nEXPR FOR ALL #[Elt] {body}");
        let (closed, under, holding) = resumed(&source, |door| {
            let f = subject(door.root());
            let elt = unknown(door.symbols(), f, "Elt");
            let at = slot(door.symbols(), f, "y");
            let Some(DeclaredType::Type(load)) = door.load().binder_type(door.types, f, at) else {
                panic!("`y` is typed, and no scheme");
            };
            let bound = door.declared("Bound");
            let levels: Vec<(usize, KType)> = (door.load().homes(f).iter())
                .filter_map(|(level, target)| match target {
                    crate::scope::Target::Local(slot)
                        if (Unknown {
                            segment: f.segment(),
                            at: Address::Slot(*slot),
                        }) == elt =>
                    {
                        Some((*level, bound))
                    }
                    _ => None,
                })
                .collect();
            let show = |ty| {
                let closed = laws::substitute(door, &levels, ty).expect("the binding closes it");
                display_name(closed, door.types, door.symbols()).to_string()
            };
            let closed = format!("[{}, {}]", show(load.lower), show(load.upper));
            let holding = (waits_on(door, unit_of(door, f, "y"), elt)
                .into_iter()
                .flatten())
            .flatten()
            .filter(|kind| kind.holds())
            .collect::<Vec<_>>();
            let binding = door.binding(&[(elt, bound)]);
            let resumed = (door.resume(None, binding, false)).expect("no refusal");
            (closed, ends(door, resumed, f, "y"), holding)
        });
        assert_eq!(closed, under, "{body} under `Elt := {declared}`");
        assert_eq!(under, expected, "{body} under `Elt := {declared}`");
        assert!(holding.is_empty(), "{body}: {holding:?}");
        // Declared `:Module`, `x` is a module binder of its own, which a restart types exactly.
        if declared != ":Module" {
            let restart = format!("{prefix}LET Elt = {declared}\nEXPR {body}");
            let restarted = resumed(&restart, |door| {
                ends(door, door.load(), subject(door.root()), "y")
            });
            assert_eq!(under, restarted, "{body} under `Elt := {declared}`");
        }
    }
}

/// A path through a module parameter whose declared application pins its label over a run-bound
/// name is that pin: under a binding of the name it is what a fresh load with the name declared
/// reads, no question it asks holds, and a second binding keeping every answer lays no unit for
/// it.
#[test]
fn a_path_pinned_over_a_run_bound_name_is_its_pin() {
    let prelude = "SIG Paired FOR ALL #[Left Right] = #[(VAL left :Left) (VAL right :Right)]\n";
    let body = "#(WRAP x :Elt AND p :(Paired WITH {Left = Elt})) -> Any = #(\n  \
                LET y :(p.Left) = p.left\n  LET Lefts = :(p.Left)\n  y\n)\n";
    let source = format!("{prelude}EXPR FOR ALL #[Elt] {body}");
    let (under, holding, laid) = resumed(&source, |door| {
        let f = subject(door.root());
        let elt = unknown(door.symbols(), f, "Elt");
        let unit = unit_of(door, f, "y");
        let holding = (waits_on(door, unit, elt).into_iter().flatten().flatten())
            .filter(|kind| kind.holds())
            .collect::<Vec<_>>();
        let under = |at| door.binding(&[(elt, at)]);
        let first = (door.resumption(under(KType::NUMBER), None, false)).expect("no refusal");
        let witnessed = first.witnessed.expect("the first resumption witnesses");
        let second = door.resumption(under(KType::STR), Some(witnessed), false);
        let laid = second.expect("no refusal").layer.unit_of(unit).is_some();
        let resumed = (door.resume(None, under(KType::NUMBER), false)).expect("no refusal");
        (ends(door, resumed, f, "y"), holding, laid)
    });
    let restarted = resumed(&format!("{prelude}LET Elt = Number\nEXPR {body}"), |door| {
        ends(door, door.load(), subject(door.root()), "y")
    });
    assert_eq!(under, restarted);
    assert_eq!(under, "[Number, Number]");
    assert!(holding.is_empty(), "{holding:?}");
    assert!(!laid, "a binding keeping every answer lays no unit for `y`");
}

/// A head a `USING` surfaces through an ascription pinned over a run-bound name registers at its
/// pins over the name: under a binding of the name the call is what a fresh load with the name
/// declared selects, no question it asks holds, and a second binding keeping every answer lays no
/// unit for it.
#[test]
fn a_head_surfaced_through_an_ascription_over_a_name_registers_over_it() {
    let prelude = PICKS;
    let body = "#(SUBJECT x :Elt AND p :Picks) -> Any = #(\n  \
                LET s0 = (USING (p :! (Picks WITH {Carrier = Elt})) SCOPE (PICK zero))\n  s0\n)\n";
    let source = format!("{prelude}EXPR FOR ALL #[Elt] {body}");
    let (load, under, holding, laid) = resumed(&source, |door| {
        let f = subject(door.root());
        let elt = unknown(door.symbols(), f, "Elt");
        let unit = unit_of(door, f, "s0");
        let holding = (waits_on(door, unit, elt).into_iter().flatten().flatten())
            .filter(|kind| kind.holds())
            .collect::<Vec<_>>();
        let under = |at| door.binding(&[(elt, at)]);
        let first = (door.resumption(under(KType::NUMBER), None, false)).expect("no refusal");
        let witnessed = first.witnessed.expect("the first resumption witnesses");
        let second = door.resumption(under(KType::STR), Some(witnessed), false);
        let laid = second.expect("no refusal").layer.unit_of(unit).is_some();
        let resumed = (door.resume(None, under(KType::NUMBER), false)).expect("no refusal");
        let load = upper(door, door.load(), f, "s0");
        (load, ends(door, resumed, f, "s0"), holding, laid)
    });
    let restarted = resumed(&format!("{prelude}LET Elt = Number\nEXPR {body}"), |door| {
        ends(door, door.load(), subject(door.root()), "s0")
    });
    assert!(
        !load.contains("Any"),
        "the load registers the head over `Elt`: {load}"
    );
    assert_eq!(under, restarted);
    assert_eq!(under, "[Str, Str]");
    assert!(holding.is_empty(), "{holding:?}");
    assert!(
        !laid,
        "a binding keeping every answer lays no unit for `s0`"
    );
}

/// A `MODULE` body whose members are written over a run-bound name is exactly the signature they
/// declare, written over the name: under a binding of the name it is what a fresh load with the
/// name declared types it, no question it asks holds, and a second binding keeping every answer
/// lays no unit for it.
#[test]
fn a_module_over_a_run_bound_name_is_its_signature_over_the_name() {
    let body = "#(SUBJECT x :Elt) -> Any = #(\n  \
                MODULE md0 OVER #[Elt] = (LET f = (FN :{y :Elt} -> Elt = #(y)))\n  md0\n)\n";
    let source = format!("EXPR FOR ALL #[Elt] {body}");
    let (under, holding, laid) = resumed(&source, |door| {
        let f = subject(door.root());
        let elt = unknown(door.symbols(), f, "Elt");
        let unit = unit_of(door, f, "md0");
        let holding = (waits_on(door, unit, elt).into_iter().flatten().flatten())
            .filter(|kind| kind.holds())
            .collect::<Vec<_>>();
        let under = |at| door.binding(&[(elt, at)]);
        let first = (door.resumption(under(KType::NUMBER), None, false)).expect("no refusal");
        let witnessed = first.witnessed.expect("the first resumption witnesses");
        let second = door.resumption(under(KType::STR), Some(witnessed), false);
        let laid = second.expect("no refusal").layer.unit_of(unit).is_some();
        let resumed = (door.resume(None, under(KType::NUMBER), false)).expect("no refusal");
        (ends(door, resumed, f, "md0"), holding, laid)
    });
    let restarted = resumed(&format!("LET Elt = Number\nEXPR {body}"), |door| {
        ends(door, door.load(), subject(door.root()), "md0")
    });
    assert_eq!(under, restarted);
    assert!(holding.is_empty(), "{holding:?}");
    assert!(
        !laid,
        "a binding keeping every answer lays no unit for `md0`"
    );
}

/// A `MODULE` body capturing a run-bound name, or a local alias written over one, holds a content
/// tree a binding closes: under a closed walk, and under a binding over a witnessed layer that
/// keeps every answer, it is closed, and over an alias it is the tree a fresh load with the name
/// declared reads.
#[test]
fn a_module_capturing_a_run_bound_name_closes_its_tree_under_a_binding() {
    let bodies = [
        "#(SUBJECT x :Elt) -> Any = #(\n  MODULE md0 OVER #[Elt] = (LET f = 7)\n  md0\n)\n",
        "#(SUBJECT x :Elt) -> Any = #(\n  LET Row = :(LIST OF Elt)\n  \
         MODULE md0 OVER #[Row] = (LET f = (FN :{y :Row} -> Row = #(y)))\n  md0\n)\n",
    ];
    for body in bodies {
        let (load, walked, second) = resumed(&format!("EXPR FOR ALL #[Elt] {body}"), |door| {
            let f = subject(door.root());
            let (elt, at) = (
                unknown(door.symbols(), f, "Elt"),
                slot(door.symbols(), f, "md0"),
            );
            let under = |ty| door.binding(&[(elt, ty)]);
            let held = "`md0` holds a content tree";
            let load = door.load().content(f, at).expect(held);
            let closed = (door.resume(None, under(KType::NUMBER), false)).expect("no refusal");
            let walked = closed.content(f, at).expect(held).identity();
            let first = (door.resumption(under(KType::STR), None, false)).expect("no refusal");
            let witnessed = first.witnessed.expect("the first resumption witnesses");
            let second = door.resumption(under(KType::NUMBER), Some(witnessed), false);
            let layer = resident(door.bump, second.expect("no refusal").layer);
            let facts = crate::scope::Facts::under(door.load(), Some(layer));
            let second = facts.content(f, at).expect(held).identity();
            (load.closed(), walked, second)
        });
        assert!(!load, "the load's tree is open over `Elt`:\n{body}");
        assert!(matches!(walked, Identity::Closed(_)), "{walked:?}\n{body}");
        assert_eq!(walked, second, "{body}");
        if body.contains("Row") {
            let restarted = resumed(&format!("LET Elt = Number\nEXPR {body}"), |door| {
                let f = subject(door.root());
                let at = slot(door.symbols(), f, "md0");
                door.load()
                    .content(f, at)
                    .expect("`md0` holds a content tree")
                    .identity()
            });
            assert_eq!(walked, restarted, "{body}");
        }
    }
}

/// A module parameter held in place at its own declared type is the parameter's own type: one
/// type, over one class.
#[test]
fn a_module_held_in_place_at_its_own_type_is_the_parameter_s_type() {
    let source = format!(
        "{COUNTER}LET step = (FN :{{m :Counter}} -> Any = #(\n  LET s0 = (m :! Counter)\n  \
         LET s1 = m\n  s0\n))\n"
    );
    let (held, read) = resumed(&source, |door| {
        let step = born(door.symbols(), door.root(), "step");
        (
            upper(door, door.load(), step, "s0"),
            upper(door, door.load(), step, "s1"),
        )
    });
    assert!(held.ends_with("WITH {Carrier = Carrier}"), "{held}");
    assert_eq!(held, read);
}

/// A module parameter held in place at a type its binding may not fit is refused by the binding
/// exactly where a fresh load, with the parameter declared at the bound module, refuses it.
#[test]
fn a_module_held_in_place_is_refused_by_a_binding_as_a_fresh_load_refuses_it() {
    let prelude = format!(
        "{COUNTER}MODULE ints = ((LET Carrier = Number) (LET zero = 0) \
         (LET succ = (FN :{{x :Number}} -> Number = #(x))))\n\
         LET stamped :Counter = ints\n\
         MODULE strs = ((LET Carrier = Str) (LET zero = \"z\") \
         (LET succ = (FN :{{x :Str}} -> Str = #(x))))\n\
         LET sstamped :Counter = strs\n\
         SIG Zeroed = #[(VAL zero :Number)]\n"
    );
    // Whether a binding of `m` to `stamped`, then to `sstamped`, admits the hold.
    for (held, fits) in [
        ("(Counter WITH {Carrier = Str})", [false, true]),
        ("Zeroed", [true, false]),
        ("(Counter & Zeroed)", [true, false]),
    ] {
        let body = format!("LET s0 = (m :! {held}).zero");
        let source =
            format!("{prelude}LET step = (FN :{{m :Counter}} -> Any = #(\n  {body}\n  s0\n))\n");
        for (module, fits) in ["stamped", "sstamped"].into_iter().zip(fits) {
            let resumed = resumed(&source, |door| {
                let step = born(door.symbols(), door.root(), "step");
                let m = unknown(door.symbols(), step, "m");
                let binding = door.bound(&[(m, door.module(module))]);
                door.resume(None, binding, false).is_ok()
            });
            let restart = format!(
                "{prelude}LET step = (FN :{{n :Any}} -> Any = #(\n  LET m = {module}\n  \
                 {body}\n  s0\n))\n"
            );
            let loaded = CellSubstrate::load::<Koan>(
                &restart,
                "<test>",
                8,
                BuildBudget::default(),
                output(),
            )
            .is_ok();
            assert_eq!(resumed, loaded, "{body} under `m := {module}`");
            assert_eq!(resumed, fits, "{body} under `m := {module}`");
        }
    }
}

/// A record holding a content tree with a leaf on a module parameter names the parameter: a
/// binding of it absorbs the record, and its tree reads as the bound module's, as a fresh load reads
/// it, through the binding's layer.
#[test]
fn binding_a_module_parameter_substitutes_it_into_the_trees_its_records_hold() {
    let source = format!(
        "{COUNTER}MODULE ints = ((LET Carrier = Number) (LET zero = 0) \
         (LET succ = (FN :{{x :Number}} -> Number = #(x))))\n\
         LET stamped :Counter = ints\n\
         LET step = (FN :{{m :Counter}} -> Any = #(\n  LET n = m\n  n\n))\n"
    );
    let (before, after, bound, n_reran) = resumed(&source, |door| {
        let symbols = door.symbols();
        let step = born(symbols, door.root(), "step");
        let n = slot(symbols, step, "n");
        let before = door.load().content(step, n).map(|tree| tree.closed());
        let stamped = door.module("stamped");
        let bound = match stamped {
            Bound::Module { tree, .. } => tree.identity(),
            Bound::Type(_) => unreachable!("`stamped` is a module"),
        };
        let binding = door.bound(&[(unknown(symbols, step, "m"), stamped)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        (
            before,
            resumed.content(step, n).map(|tree| tree.identity()),
            bound,
            reran(resumed, unit_of(door, step, "n")),
        )
    });
    assert_eq!(before, Some(false), "the load reads `n` as a leaf on `m`");
    assert_eq!(
        after,
        Some(bound),
        "a binding reads `n` as the module it binds"
    );
    assert!(!n_reran, "`n`'s record names `m`, and is absorbed");
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
        let f = quantified(door);
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
        let f = quantified(door);
        let binding = door.binding(&[(unknown(door.symbols(), f, "Elt"), KType::NUMBER)]);
        door.resume(None, binding, false).map(|_| ())
    });
    assert_eq!(resumed, Ok(()));
}

/// A binding re-runs exactly the units whose frontier reaches an open mark on a name it binds. The
/// entry unit seeds `Key` at the decided name of its kind's type value, which asks nothing, so it
/// only names `Key`; `WRAP`'s other units read `x`'s seed alone. So binding `Key` re-runs nothing.
#[test]
fn binding_a_name_no_fact_reads_re_runs_nothing() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt Key] #(WRAP x :Elt) -> Any = #(\n  LET p = (PICK x)\n  p\n)"
    );
    let (reran, waiting) = resumed(&source, |door| {
        let f = quantified(door);
        let key = unknown(door.symbols(), f, "Key");
        let binding = door.binding(&[(key, KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let reran: Vec<_> = (resumed.own_units()).map(|(unit, _)| unit).collect();
        let named: Vec<_> = (door.load().own_units())
            .filter(|(_, (_, frontier))| frontier.unknowns().contains(&key))
            .map(|(unit, _)| unit)
            .collect();
        let entry = crate::scope::UnitId {
            segment: f.segment(),
            unit: crate::scope::UnitIndex::ENTRY,
        };
        (reran, (named, entry))
    });
    let (named, entry) = waiting;
    assert_eq!(
        reran,
        vec![],
        "`Key`'s seed asks nothing, so nothing re-runs"
    );
    assert_eq!(named, vec![entry], "no unit but the entry reaches `Key`");
}

/// A binding under which a quote's code refuses leaves no record of the code's units: no `OP`
/// takes a `Bool`, so the resumption marks the unit the load typed unreached, and a reader finds
/// none, as in a fresh load with `Elt` bound.
#[test]
fn a_binding_refusing_a_quote_s_code_hides_the_load_s_records_of_it() {
    let source = "EXPR #(OP x :Number) -> Str = #(\"n\")\nEXPR #(OP x :Str) -> Number = #(1)\n\
        EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
        LET s0 = (EVAL #($(OP $x)) -> Any)\n  x\n)";
    let (loaded, read, cut, refused) = resumed(source, |door| {
        let f = quantified(door);
        let (_, code) = *(f.nested_shapes().iter())
            .find(|(_, nested)| nested.kind() == ShapeKind::Code)
            .expect("SUBJECT quotes a code");
        let unit = crate::scope::UnitId {
            segment: code.segment(),
            unit: crate::scope::UnitIndex::of_run(0),
        };
        let binding = door.binding(&[(unknown(door.symbols(), f, "Elt"), KType::BOOL)]);
        let resumed = (door.resume(None, binding, false)).expect("only the code refuses");
        let refused =
            (resumed.closing(code.segment())).is_some_and(|(closing, _)| closing.refusal.is_some());
        (
            door.load().unit(unit).is_some(),
            resumed.unit(unit).is_some(),
            resumed.unreached_units().any(|at| at == unit),
            refused,
        )
    });
    assert!(loaded, "the load types the code's unit");
    assert!(refused, "under `Bool` the code refuses");
    assert!(cut, "the resumption marks the code's unit unreached");
    assert!(!read, "a reader finds no record of it");
}

/// `WRAP`'s body: `y` holds `x :Elt` to `Number`, which is `Never` where `Elt` is, and `z` reads
/// `y` alone.
const ARRIVES: &str = "#(WRAP x :Elt) -> Any = #(\n  LET y :Number = x\n  LET z = y\n  z\n)";

/// A value held to a type is `Never` where it never arrives: at the load, the decided name over
/// whether `Elt` is `Never`, so a binding of `Elt` to `Never` gives `y` and `z`, which only reads
/// `y`, the types a restart with `Elt` declared gives them, and one to `Number` gives `Number`.
#[test]
fn a_value_that_never_arrives_is_never_under_the_binding_that_makes_it_so() {
    for (bound, declared) in [(KType::NEVER, ":Never"), (KType::NUMBER, ":Number")] {
        let (load, under) = resumed(&format!("EXPR FOR ALL #[Elt] {ARRIVES}"), |door| {
            let f = quantified(door);
            let elt = unknown(door.symbols(), f, "Elt");
            let binding = door.binding(&[(elt, bound)]);
            let resumed =
                (door.resume(None, binding, false)).expect("the resumption refuses nothing");
            let read = |facts| ["y", "z"].map(|name| upper(door, facts, f, name));
            (read(door.load()), read(resumed))
        });
        let restart = format!("LET Elt = {declared}\nEXPR {ARRIVES}");
        let restarted = resumed(&restart, |door| {
            let f = subject(door.root());
            ["y", "z"].map(|name| upper(door, door.load(), f, name))
        });
        assert_eq!(
            load, ["(ONE OF Number, Never)"; 2],
            "`y` arrives where `Elt` is not `Never`"
        );
        assert_eq!(under, restarted, "under `Elt := {declared}`");
    }
}

/// Two `FIRST` overloads: one over a record whose field `x` is `Elt`, one at `Number`.
const FIRSTS: &str = "EXPR FOR ALL #[Elt] #(FIRST r :{x :Elt}) -> Elt = #(r.x)\n\
                      EXPR #(FIRST n :Number) -> Number = #(n)\n";

/// `SUBJECT`'s body: `s` calls `FIRST` over `y :Item`, and `t` reads `s` alone.
const SELECTS: &str = "#(SUBJECT y :Item) -> Any = #(\n  LET s = (FIRST y)\n  LET t = s\n  t\n)";

/// Both ends of the binder `name` of `shape`, as `facts` holds it, rendered.
fn ends(door: &Door<'_, '_>, facts: &LoadFacts<'_>, shape: &BodyShape<'_>, name: &str) -> String {
    let at = slot(door.symbols(), shape, name);
    let Some(DeclaredType::Type(typed)) = facts.binder_type(door.types, shape, at) else {
        panic!("`{name}` is typed, and no scheme");
    };
    let render = |ty| display_name(ty, door.types, door.symbols()).to_string();
    format!("[{}, {}]", render(typed.lower), render(typed.upper))
}

/// A call keeping two candidates over `y :Item` is typed by which runs, one decided name per
/// candidate: a binding of `Item` gives `s` and `t` what a fresh load with `Item` declared
/// selects — the record overload under a record, the `Number` one under `Number`, and both at
/// most under `Any`, where the call reads the carried type.
#[test]
fn a_call_s_type_under_a_binding_is_what_a_fresh_load_selects() {
    for declared in [":{x :Str}", ":Number", ":Any"] {
        let source = format!("LET Bound = {declared}\n{FIRSTS}EXPR FOR ALL #[Item] {SELECTS}");
        let under = resumed(&source, |door| {
            let f = subject(door.root());
            let item = unknown(door.symbols(), f, "Item");
            let binding = door.binding(&[(item, door.declared("Bound"))]);
            let resumed =
                (door.resume(None, binding, false)).expect("the resumption refuses nothing");
            ["s", "t"].map(|name| ends(door, resumed, f, name))
        });
        let restart = format!("{FIRSTS}LET Item = {declared}\nEXPR {SELECTS}");
        let restarted = resumed(&restart, |door| {
            let f = subject(door.root());
            ["s", "t"].map(|name| ends(door, door.load(), f, name))
        });
        assert_eq!(under, restarted, "under `Item := {declared}`");
    }
}

/// `SUBJECT`'s body: `m` matches `y :Item` against a `Number` arm and `_`, and `t` reads `m` alone.
const MATCHES: &str = "#(SUBJECT y :Item) -> Any = #(\n  \
                       LET m = (MATCH y WITH #{:Number: (1), _: (\"s\")})\n  LET t = m\n  t\n)";

/// A `MATCH` over `y :Item` is typed by which arms run, one decided name per arm: a binding of
/// `Item` gives `m` and `t` what a fresh load with `Item` declared gives them — the `Number` arm's
/// type alone under `Number`, `_`'s under `Str`, and both at most under `Any`.
#[test]
fn a_match_s_type_under_a_binding_is_what_a_fresh_load_takes() {
    for declared in [":Number", ":Str", ":Any"] {
        let source = format!("LET Bound = {declared}\nEXPR FOR ALL #[Item] {MATCHES}");
        let (load, under) = resumed(&source, |door| {
            let f = subject(door.root());
            let item = unknown(door.symbols(), f, "Item");
            let binding = door.binding(&[(item, door.declared("Bound"))]);
            let resumed =
                (door.resume(None, binding, false)).expect("the resumption refuses nothing");
            let read = |facts| ["m", "t"].map(|name| ends(door, facts, f, name));
            (read(door.load()), read(resumed))
        });
        let restart = format!("LET Item = {declared}\nEXPR {MATCHES}");
        let restarted = resumed(&restart, |door| {
            let f = subject(door.root());
            ["m", "t"].map(|name| ends(door, door.load(), f, name))
        });
        assert_ne!(load, restarted, "the load leaves the arm to the binding");
        assert_eq!(under, restarted, "under `Item := {declared}`");
    }
}

/// `APPLY`, whose second slot solves `Elt` from a function's return, beside a module whose `pick`
/// is quantified.
const APPLIES: &str = "EXPR #(APPLY 2 TO 1)\n\
     EXPR FOR ALL #[Elt] #(APPLY f :(FN :{x :Elt} -> Elt) TO y :(FN :{x :Number} -> Elt)) \
     -> Elt = #(f {x = 1})\n\
     MODULE lib = (LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x)))\n";

/// `SUBJECT`'s body: `s` applies `lib.pick` to `g :Outer`, and `t` reads `s` alone.
const STORED: &str = "#(SUBJECT g :Outer) -> Any = #(\n  LET s = (APPLY lib.pick TO g)\n  \
                      LET t = s\n  t\n)";

/// `Outer` is read through its bound, which a binding lower solves otherwise, so the load stores
/// `APPLY`'s solve whole and fixes `pick` at the derived name it types: a binding of `Outer` gives
/// `s` and `t` what a fresh load with `Outer` declared gives them.
#[test]
fn a_stored_solve_fixes_an_instance_its_binding_solves() {
    let declared = ":(FN :{x :Number} -> Number)";
    let source = format!(
        "LET Bound = {declared}\n{APPLIES}\
         EXPR FOR ALL #{{Outer: :(FN :{{x :Number}} -> Number)}} {STORED}"
    );
    let (load, under) = resumed(&source, |door| {
        let f = subject(door.root());
        let outer = unknown(door.symbols(), f, "Outer");
        let binding = door.binding(&[(outer, door.declared("Bound"))]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let read = |facts| ["s", "t"].map(|name| ends(door, facts, f, name));
        (read(door.load()), read(resumed))
    });
    let restart = format!("{APPLIES}LET Outer = {declared}\nEXPR {STORED}");
    let restarted = resumed(&restart, |door| {
        let f = subject(door.root());
        ["s", "t"].map(|name| ends(door, door.load(), f, name))
    });
    assert_ne!(load, restarted, "the load leaves `s` to the binding");
    assert_eq!(under, restarted);
}

/// Three `OP` overloads, one quantified over the element of a list.
const OPS: &str = "EXPR #(OP x :Number) -> Str = #(\"n\")\n\
                   EXPR #(OP x :Str) -> Number = #(1)\n\
                   EXPR FOR ALL #[Elt] #(OP x :(LIST OF Elt)) -> :(LIST OF Elt) = #(x)\n";

/// `SUBJECT`'s body: `s` calls `OP` over `y :Item`, and `t` reads `s` alone.
const JUDGED: &str = "#(SUBJECT y :Item) -> Any = #(\n  LET s = (OP y)\n  LET t = s\n  t\n)";

/// The list overload's group solved from `y :Item` is a solve a run may solve otherwise, so its
/// verdict's intervals are derived names judged again under a binding: under `Item := (LIST OF
/// Number)`, `s` and `t` are what a fresh load with `Item` declared gives them.
#[test]
fn a_verdict_s_unreproduced_intervals_are_derived_names() {
    let declared = ":(LIST OF Number)";
    let source = format!("LET Bound = {declared}\n{OPS}EXPR FOR ALL #[Item] {JUDGED}");
    let (load, under) = resumed(&source, |door| {
        let f = subject(door.root());
        let item = unknown(door.symbols(), f, "Item");
        let binding = door.binding(&[(item, door.declared("Bound"))]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let read = |facts| ["s", "t"].map(|name| ends(door, facts, f, name));
        (read(door.load()), read(resumed))
    });
    let restart = format!("{OPS}LET Item = {declared}\nEXPR {JUDGED}");
    let restarted = resumed(&restart, |door| {
        let f = subject(door.root());
        ["s", "t"].map(|name| ends(door, door.load(), f, name))
    });
    assert!(
        load[0].contains("JUDGED"),
        "the load reads the judged names: {load:?}"
    );
    assert_eq!(under, restarted);
}

/// A family whose `Of` variant holds a function returning its parameter.
const RET: &str = "UNION (Elem -> Ret) = #{Of: :(FN :{x :Number} -> Elem), None: Null}\n";

/// `SUBJECT`'s body: `s` constructs `Ret.Of` over `g :Outer`, and `t` reads `s` alone.
const CONSTRUCTS: &str = "#(SUBJECT g :Outer) -> Any = #(\n  LET s = (Ret.Of g)\n  \
                          LET t = s\n  t\n)";

/// `Elem` is solved from `Outer`'s return through its bound, which a binding lower solves
/// otherwise, so the construction is the decided name on whether it solves, typed by the
/// construction's own: a binding of `Outer` gives `s` and `t` what a fresh load gives them.
#[test]
fn a_construction_a_run_may_solve_otherwise_is_a_derived_name() {
    let declared = ":(FN :{x :Number} -> Number)";
    let source = format!(
        "LET Bound = {declared}\n{RET}\
         EXPR FOR ALL #{{Outer: :(FN :{{x :Number}} -> Number)}} {CONSTRUCTS}"
    );
    let (load, under) = resumed(&source, |door| {
        let f = subject(door.root());
        let outer = unknown(door.symbols(), f, "Outer");
        let binding = door.binding(&[(outer, door.declared("Bound"))]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let read = |facts| ["s", "t"].map(|name| ends(door, facts, f, name));
        (read(door.load()), read(resumed))
    });
    let restart = format!("{RET}LET Outer = {declared}\nEXPR {CONSTRUCTS}");
    let restarted = resumed(&restart, |door| {
        let f = subject(door.root());
        ["s", "t"].map(|name| ends(door, door.load(), f, name))
    });
    assert!(
        load[0].contains("CONSTRUCTED"),
        "the load reads the construction's name: {load:?}"
    );
    assert_eq!(under, restarted);
}

/// `SUBJECT`'s body: `f` is a quantified `FN` wanted at a function type over `Outer`, and `t`
/// reads `f` alone.
const INSTANCES: &str = "#(SUBJECT g :Outer) -> Any = #(\n  \
     LET f :(FN :{x :Outer} -> Number) = (FN FOR ALL #[Pick] :{x :Pick} -> Pick = #(x))\n  \
     LET t = f\n  t\n)";

/// The instance `f` takes is solved over `Outer`, a solve a binding may make otherwise, so the
/// load fixes it at each variable's derived name, instantiated again under a binding: under
/// `Outer := Number`, `f` and `t` are what a fresh load with `Outer` declared gives them.
#[test]
fn an_instance_a_run_may_make_otherwise_is_its_variables_derived_names() {
    let source = format!("EXPR FOR ALL #[Outer] {INSTANCES}");
    let (spelled, under) = resumed(&source, |door| {
        let f = subject(door.root());
        let outer = unknown(door.symbols(), f, "Outer");
        let binding = door.binding(&[(outer, KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let derived = door.load().base_layer().derived();
        let spelled: Vec<String> = (derived.names().iter())
            .map(|name| display_name(name.ty, door.types, door.symbols()).to_string())
            .collect();
        (spelled, ["f", "t"].map(|name| ends(door, resumed, f, name)))
    });
    let restart = format!("LET Outer = :Number\nEXPR {INSTANCES}");
    let restarted = resumed(&restart, |door| {
        let f = subject(door.root());
        ["f", "t"].map(|name| ends(door, door.load(), f, name))
    });
    assert!(
        spelled.iter().any(|name| name.starts_with("(INSTANCE ")),
        "the load fixes the instance at a derived name: {spelled:?}"
    );
    assert_eq!(under, restarted);
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
        let f = quantified(door);
        let binding = door.binding(&[(unknown(door.symbols(), f, "Elt"), KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let porous =
            |facts: &LoadFacts<'_>| matches!(facts.class(f), Some(BodyClass::Porous { .. }));
        assert!(
            (resumed.own_units()).all(|(unit, _)| unit.unit.run_index() != Some(0)),
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
        let f = quantified(door);
        let g = born(symbols, f, "g");
        let before = narrowing(door, door.load(), g, "p");
        let binding = door.binding(&[(unknown(symbols, f, "Elt"), KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        (before, narrowing(door, resumed, g, "p"))
    });
    assert_eq!(before, "kept maybe maybe");
    assert_eq!(after, "selected");
}

/// A callable born inside a block keeps its own records: a binding its body reads re-runs it even
/// where the unit running the block reads nothing the binding binds, and is absorbed.
#[test]
fn an_inner_callable_inside_a_block_settles_under_its_binding() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
         MATCH 1 WITH #{{Number: ((LET g = (FN :{{y :Number}} -> Any = #(\
         (LET p = (PICK x)) (p)))) (g))}}\n)"
    );
    let (before, after) = resumed(&source, |door| {
        let symbols = door.symbols();
        let f = quantified(door);
        let (_, arm) = (f.nested_shapes().iter())
            .find(|(_, nested)| nested.kind() == ShapeKind::Block)
            .expect("the `MATCH` has an arm");
        let g = born(symbols, arm, "g");
        let before = narrowing(door, door.load(), g, "p");
        let binding = door.binding(&[(unknown(symbols, f, "Elt"), KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let arm_unit = (f.segment(), f.unit_of_statement(0));
        assert!(
            (resumed.own_units()).all(|(unit, _)| (unit.segment, unit.unit) != arm_unit),
            "the unit running the arm is absorbed"
        );
        (before, narrowing(door, resumed, g, "p"))
    });
    assert_eq!(before, "kept maybe maybe");
    assert_eq!(after, "selected");
}

/// A settlement rests in the agent tier what its records name, adopted from the chain its walk
/// interned through: each such type resolves there once the miss ends, and a second settlement
/// under the same binding adds nothing.
#[test]
fn the_agent_tier_grows_by_the_types_a_settlements_records_name() {
    let source = "LET Pt = :{x :Number}\n\
                  EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #((LET l = [x]) (l))\n";
    let (grew, again, unresolved) = resumed(source, |door| {
        let wrap = quantified(door);
        let ask = crate::scheduler::Ask {
            body: wrap.segment(),
            bound: [crate::scheduler::Asked::Type {
                slot: slot(door.symbols(), wrap, "Elt"),
                named: door.declared("Pt"),
            }]
            .into_iter()
            .collect(),
        };
        let agent = door.running.program().tiers().agent;
        let before = agent.len();
        let facts = (door.running.settle::<Koan>(&ask, door.bump)).expect("`Pt` is a type");
        let grew = agent.len() - before;
        let resting = door.running.program().tiers().resting();
        let mut unresolved = 0;
        (facts.top().expect("a settlement")).each_type(&mut |kt| {
            unresolved += usize::from(!resting.resolves(kt));
        });
        let before = agent.len();
        door.running.settle::<Koan>(&ask, door.bump);
        (grew, agent.len() - before, unresolved)
    });
    assert!(grew > 0, "`[x]` at `Pt` is a type the root lacks");
    assert_eq!(unresolved, 0, "every type the records name rests");
    assert_eq!(again, 0, "a type rests once");
}

#[test]
fn a_binding_no_overload_admits_refuses_as_the_load_would() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET y = (x + 1)\n  y\n)";
    let refused = resumed(source, |door| {
        let f = quantified(door);
        let binding = door.binding(&[(unknown(door.symbols(), f, "Elt"), KType::STR)]);
        door.resume(None, binding, false).map(|_| ())
    });
    let refused = refused.expect_err("no overload admits `Str`");
    assert!(
        refused.contains(
            "no overload of `_ + _` admits these arguments:\n  Str, which can never satisfy Number"
        ),
        "{refused}"
    );
}

#[test]
fn a_type_binder_over_a_bound_name_declares_closed() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
                  NEWTYPE Box = :{v :Elt}\n  (Box {v = x})\n)";
    let (before, after) = resumed(source, |door| {
        let f = quantified(door);
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
        let f = quantified(door);
        let binding = door.binding(&[(unknown(symbols, f, "Elt"), KType::NUMBER)]);
        let render = |everything| {
            let resumed =
                (door.resume(None, binding, everything)).expect("the resumption refuses nothing");
            (
                narrowing(door, resumed, f, "p"),
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
    crate::scope::each_part(&shape.body()[index], |part| sites.push(shape.site(part)));
    sites.push(shape.site_of_node(&shape.body()[index]));
    (sites.into_iter())
        .filter(|site| facts.settled(shape, *site))
        .count()
}

#[test]
fn a_check_a_binding_no_longer_settles_is_dropped() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET s :Elt = x\n  s\n)";
    let (load, skipping, everything) = resumed(source, |door| {
        let f = quantified(door);
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
        let f = quantified(door);
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
        let f = quantified(door);
        let (elt, key) = (unknown(symbols, f, "Elt"), unknown(symbols, f, "Key"));
        let once = door.binding(&[(elt, KType::NUMBER)]);
        let first = (door.resume(None, once, false)).expect("the resumption refuses nothing");
        let both = door.binding(&[(elt, KType::NUMBER), (key, KType::STR)]);
        let second =
            (door.resume(Some(first), both, false)).expect("the resumption refuses nothing");
        let render = |facts| {
            (
                narrowing(door, facts, f, "p"),
                narrowing(door, facts, f, "q"),
            )
        };
        (render(first), render(second))
    });
    assert_eq!(
        first,
        ("selected".to_string(), "kept maybe maybe".to_string())
    );
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
        let f = quantified(door);
        let binding = door.binding(&[(unknown(door.symbols(), f, "Elt"), bound)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let p = slot(door.symbols(), f, "p");
        let bound_upper = matches!(
            resumed.binder_type(door.types, f, p),
            Some(DeclaredType::Type(typed)) if typed.upper == bound.into()
        );
        let pair = slot(door.symbols(), f, "Pair");
        let adopted = matches!(
            resumed.declared_type(door.types, f, pair),
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
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        door.resume(None, door.binding(&[(elt, KType::NUMBER)]), false)
            .map(|_| ())
    });
    let refused = refused.expect_err("`Number` overlaps the builtin");
    assert!(
        refused.contains("this overload of `_ + _` takes operands the builtin"),
        "{refused}"
    );
    // Under `Str` it overlaps no overload of `+`, so the resumption refuses nothing.
    let source = "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  OP #(+) OVER Elt = #(x)\n  x\n)";
    let resumed = resumed(source, |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        door.resume(None, door.binding(&[(elt, KType::STR)]), false)
            .map(|_| ())
    });
    assert_eq!(resumed, Ok(()));
}

/// A registration over a name whose bound already misses every builtin overload at its key never
/// overlaps one: the check asks nothing.
#[test]
fn a_registration_whose_bound_misses_the_builtins_asks_nothing() {
    let source = "EXPR FOR ALL #{Elt: Str} #(SUBJECT x :Elt) -> Any = #(\n  \
                  OP #(+) OVER Elt = #(x)\n  x\n)";
    let marked = resumed(source, |door| {
        (door.load().own_units()).any(|(_, (_, frontier))| {
            (frontier.marks().iter()).any(|noted| {
                matches!(noted.mark, crate::scope::Mark::Open(open)
                    if open.kind == crate::scope::OpenKind::Overlap)
            })
        })
    });
    assert!(!marked, "no unit is marked for an overlap");
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
        let f = quantified(door);
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

/// A quote's code read under a binding of its `$` capture decides what reads it: the load types
/// `$r.zero` over the type the module `$r` holds carries, which binding `$r` to `ints` in the
/// code's activation closes.
#[test]
fn an_eval_frame_settles_facts_waiting_on_its_dollar_names_in_its_activation() {
    let source = "SIG Counter FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                  MODULE ints = ((LET zero = 0) \
                  (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n\
                  EXPR #(USE r :Counter) -> Any = #(EVAL #($r.zero) -> Any)\n";
    resumed(source, |door| {
        let code = (door.running.program().shapes().bodies().iter())
            .find(|shape| shape.kind() == ShapeKind::Code)
            .expect("the program quotes code");
        let capture = (code.captures().iter())
            .position(|capture| door.symbols().display(capture.name.symbol()).to_string() == "r")
            .expect("the code captures `r`");
        let loaded =
            (door.load().statement_type(door.types, code, 0)).expect("the load types `$r.zero`");
        assert!(!loaded.is_exact(), "the load leaves `$r.zero` open");
        let own = door.bound(&[(
            Unknown {
                segment: code.segment(),
                at: Address::Capture(crate::scope::CaptureSlot(capture as u32)),
            },
            door.module("ints"),
        )]);
        let facts = (door.running)
            .activate::<Koan>(door.types, own, code.segment(), door.bump)
            .expect("the activation types the code");
        let decided =
            (facts.statement_type(door.types, code, 0)).expect("the activation types `$r.zero`");
        assert_eq!(
            decided,
            crate::type_lattice::Interval::point(KType::NUMBER.into())
        );
    });
}

/// A quote's `$` capture of a module binder is bound as its source: resuming `USE` with `r` bound
/// to `ints` types the code's `$r.zero` as the activation binding `$r` itself does, and the load
/// types it over the capture alone, whatever `r` binds.
#[test]
fn a_binding_of_a_captured_module_binder_closes_its_capture_as_its_source() {
    let source = "SIG Counter FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                  MODULE ints = ((LET zero = 0) \
                  (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n\
                  EXPR #(USE r :Counter) -> Any = #(EVAL #($r.zero) -> Any)\n";
    resumed(source, |door| {
        let code = (door.running.program().shapes().bodies().iter())
            .find(|shape| shape.kind() == ShapeKind::Code)
            .expect("the program quotes code");
        let r = unknown(door.symbols(), subject(door.root()), "r");
        let resumed = (door.resume(None, door.bound(&[(r, door.module("ints"))]), false))
            .expect("the resumption refuses nothing");
        assert_eq!(
            resumed.statement_type(door.types, code, 0),
            Some(crate::type_lattice::Interval::point(KType::NUMBER.into()))
        );
    });
}

/// `SUBJECT`'s body: the last callable the top level births.
pub(super) fn subject<'graph>(root: &'graph BodyShape<'graph>) -> &'graph BodyShape<'graph> {
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
    shape.site_of_node(node)
}

/// A module unknown binds to a closed content tree, so a view over a bound module parameter is
/// over a closed tree: the load sees an open one, and the view's record, absorbed under the
/// binding, reads the closed tree the binding substitutes, as a fresh load builds it.
#[test]
fn a_view_over_a_bound_module_is_settled() {
    let source = format!(
        "{COUNTER}MODULE bound = ((LET Carrier = Number) (LET zero = 0) \
         (LET succ = (FN :{{x :Number}} -> Number = #(x))))\n\
         LET stamped :Counter = bound\n\
         EXPR #(SUBJECT m :Counter) -> Any = #(\n  LET v = (m :| Counter)\n  v\n)\n"
    );
    let (mint, load, resumed, v_reran) = resumed(&source, |door| {
        let body = subject(door.root());
        let site = node_of(door.symbols(), body, "v");
        let m = unknown(door.symbols(), body, "m");
        let binding = door.bound(&[(m, door.module("stamped"))]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let closed = |facts: &LoadFacts<'_>| facts.view_tree(body, site).map(|tree| tree.closed());
        (
            door.load().mint_at(body, site),
            closed(door.load()),
            closed(resumed),
            reran(resumed, unit_of(door, body, "v")),
        )
    });
    assert_eq!(mint, Some(Mint::Invariant), "the load sees an open tree");
    assert_eq!(load, Some(false));
    assert_eq!(resumed, Some(true), "the bound tree is closed");
    assert!(!v_reran, "`v`'s record names `m`, and is absorbed");
}

/// A view a binder holds over a module parameter is the view rule over the paths through its tree
/// at the load, and the view a fresh load builds under a binding of the parameter: its unit is
/// absorbed, its type closed through the carrier the substituted view hides.
#[test]
fn a_view_a_binder_holds_is_the_view_rule_under_every_binding() {
    let body = "EXPR #(SUBJECT m :Counter) -> Any = #(\n  LET w = (m :| Counter)\n  w\n)\n";
    let module = "MODULE bound = ((LET Carrier = Number) (LET zero = 0) \
                  (LET succ = (FN :{x :Number} -> Number = #(x))))\n\
                  LET stamped :Counter = bound\n";
    let source = format!("{COUNTER}{module}{body}");
    let (load, under, w_reran) = resumed(&source, |door| {
        let f = subject(door.root());
        let load = upper(door, door.load(), f, "w");
        let m = unknown(door.symbols(), f, "m");
        let binding = door.bound(&[(m, door.module("stamped"))]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let w_reran = reran(resumed, unit_of(door, f, "w"));
        (load, upper(door, resumed, f, "w"), w_reran)
    });
    let restart = format!(
        "{COUNTER}{module}EXPR #(SUBJECT n :Any) -> Any = #(\n  LET m = stamped\n  \
         LET w = (m :| Counter)\n  w\n)\n"
    );
    let restarted = resumed(&restart, |door| {
        upper(door, door.load(), subject(door.root()), "w")
    });
    assert!(load.starts_with("(VIEW OF "), "{load}");
    assert_eq!(under, restarted);
    assert!(!w_reran, "`w` names `m`, and is absorbed");
}

/// A view written in place over a module parameter is the view rule over the paths through its
/// own site at the load, and the view a fresh load builds under a binding of the parameter: its
/// statement's unit is absorbed, its type closed through the carrier the substituted view hides.
#[test]
fn a_view_written_in_place_is_the_view_rule_under_every_binding() {
    let body = "#(\n  LET k = 1\n  (m :| Counter)\n)\n";
    let module = "MODULE bound = ((LET Carrier = Number) (LET zero = 0) \
                  (LET succ = (FN :{x :Number} -> Number = #(x))))\n\
                  LET stamped :Counter = bound\n";
    let viewed = |door: &Door<'_, '_>, facts: &LoadFacts<'_>| {
        let typed = (facts.statement_type(door.types, subject(door.root()), 1))
            .expect("the view's statement is typed");
        display_name(typed.upper, door.types, door.symbols()).to_string()
    };
    let source = format!("{COUNTER}{module}EXPR #(SUBJECT m :Counter) -> Any = {body}");
    let (load, under, reran_view) = resumed(&source, |door| {
        let f = subject(door.root());
        let m = unknown(door.symbols(), f, "m");
        let binding = door.bound(&[(m, door.module("stamped"))]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let unit = crate::scope::UnitId {
            segment: f.segment(),
            unit: f.unit_of_statement(1),
        };
        (
            viewed(door, door.load()),
            viewed(door, resumed),
            reran(resumed, unit),
        )
    });
    let restart = format!(
        "{COUNTER}{module}EXPR #(SUBJECT n :Any) -> Any = \
         #(\n  LET m = stamped\n  (m :| Counter)\n)\n"
    );
    let restarted = resumed(&restart, |door| viewed(door, door.load()));
    assert!(load.starts_with("(VIEW OF "), "{load}");
    assert_eq!(under, restarted);
    assert!(!reran_view, "the view names `m`, and is absorbed");
}

/// A local reading a view over a module parameter reads the view's own paths, which a binding of
/// the parameter closes by tree substitution: the run never binds the view, so the reader marks
/// nothing on it, and under the parameter's binding is typed as a fresh load types it.
#[test]
fn a_local_reading_a_view_marks_only_what_the_view_reads() {
    let body = "LET w = (m :| Counter)\n  LET k = w.zero\n  k\n)\n";
    let module = "MODULE bound = ((LET Carrier = Number) (LET zero = 0) \
                  (LET succ = (FN :{x :Number} -> Number = #(x))))\n\
                  LET stamped :Counter = bound\n";
    let source = format!("{COUNTER}{module}EXPR #(SUBJECT m :Counter) -> Any = #(\n  {body}");
    let (under, w_marked) = resumed(&source, |door| {
        let f = subject(door.root());
        let m = unknown(door.symbols(), f, "m");
        let w = unknown(door.symbols(), f, "w");
        let w_marked = waits_on(door, unit_of(door, f, "k"), w);
        let binding = door.bound(&[(m, door.module("stamped"))]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        (upper(door, resumed, f, "k"), w_marked)
    });
    let restart =
        format!("{COUNTER}{module}EXPR #(SUBJECT n :Any) -> Any = #(\n  LET m = stamped\n  {body}");
    let restarted = resumed(&restart, |door| {
        upper(door, door.load(), subject(door.root()), "k")
    });
    assert_eq!(w_marked, None, "`k` marks nothing on `w`");
    assert_eq!(under, restarted);
}

/// A `:Module` parameter is exactly the type its argument carries, the derived name a binding of
/// it gives: a reader of it, of a list over it, of a view of it or of its member holds no read of
/// its content, and under a binding of it is typed as a fresh load types it. A reader of it or a
/// list over it names it alone and is absorbed; whether the module fits a view, or declares the
/// member read, is a question about which module the binding gives.
#[test]
fn a_module_parameter_is_the_type_its_argument_carries() {
    let module = "MODULE bound = ((LET Carrier = Number) (LET zero = 0) \
                  (LET succ = (FN :{x :Number} -> Number = #(x))))\n\
                  LET stamped :Module = bound\n";
    for (body, named) in [
        ("LET k = m\n  k\n)\n", true),
        ("LET k = [m]\n  k\n)\n", true),
        ("LET k = (m :| Counter)\n  k\n)\n", false),
        ("LET k = m.zero\n  k\n)\n", false),
    ] {
        let source = format!("{COUNTER}{module}EXPR #(SUBJECT m :Module) -> Any = #(\n  {body}");
        let (waits, under, reran_k) = resumed(&source, |door| {
            let f = subject(door.root());
            let (m, k) = (unknown(door.symbols(), f, "m"), unit_of(door, f, "k"));
            let waits = waits_on(door, k, m).expect("`k` reads `m`");
            let binding = door.bound(&[(m, door.module("stamped"))]);
            let resumed =
                (door.resume(None, binding, false)).expect("the resumption refuses nothing");
            (waits, ends(door, resumed, f, "k"), reran(resumed, k))
        });
        let restart = format!(
            "{COUNTER}{module}EXPR #(SUBJECT n :Any) -> Any = #(\n  LET m = stamped\n  {body}"
        );
        let restarted = resumed(&restart, |door| {
            ends(door, door.load(), subject(door.root()), "k")
        });
        assert!(
            !waits
                .iter()
                .any(|mark| mark.is_some_and(|kind| kind.holds())),
            "{body}: {waits:?}"
        );
        assert_eq!(under, restarted, "{body}");
        if named {
            assert!(waits.iter().all(Option::is_none), "{body}: {waits:?}");
            assert!(!reran_k, "{body}: `k` is absorbed");
        }
    }
}

/// `MAKE`, its own `FOR ALL` group's or none, nested in `SUBJECT` or not: the text selects it, so
/// `LET a = (MAKE x)` unfolds over `x`'s leaf and `a.Zero` reads `built`'s `Number`, at load and
/// under `Elt := Number` alike, as a fresh load with `x :Number` reads it.
#[test]
fn a_call_the_text_selects_unfolds_over_names() {
    let made = "(MODULE built OVER #[e] = (LET Zero = Number))\n    built\n  ";
    for (outer, inner) in [
        (
            String::new(),
            format!("EXPR #(MAKE e :Elt) -> Module = #(\n    {made})\n  "),
        ),
        (
            format!("EXPR FOR ALL #[Tee] #(MAKE e :Tee) -> Module = #(\n    {made})\n"),
            String::new(),
        ),
    ] {
        let body = |declared: &str, inner: &str| {
            format!(
                "{outer}EXPR {declared} -> Any = #(\n  {inner}LET a = (MAKE x)\n  \
                 LET z :(a.Zero) = 5\n  z\n)\n"
            )
        };
        let (loaded, under) = resumed(&body("FOR ALL #[Elt] #(SUBJECT x :Elt)", &inner), |door| {
            let f = subject(door.root());
            let elt = unknown(door.symbols(), f, "Elt");
            let binding = door.binding(&[(elt, KType::NUMBER)]);
            let resumed =
                (door.resume(None, binding, false)).expect("the resumption refuses nothing");
            (ends(door, door.load(), f, "z"), ends(door, resumed, f, "z"))
        });
        let declared = inner.replace(":Elt", ":Number");
        let restarted = resumed(&body("#(SUBJECT x :Number)", &declared), |door| {
            ends(door, door.load(), subject(door.root()), "z")
        });
        assert_eq!(loaded, "[Number, Number]", "{outer}{inner}");
        assert_eq!(under, restarted, "{outer}{inner}");
    }
}

/// A parameter over a name binds its argument retyped to it, which a binding of the name closes:
/// under `Elt := Named` the argument `x` is keyed at `Named`, as a fresh load with `x :Named` keys
/// it, though the load could not say whether the retype lays a header.
#[test]
fn an_argument_retyped_to_a_name_is_retyped_as_the_binding_closes_it() {
    let body = |declared: &str, parameter: &str| {
        format!(
            "SIG Named = #[(VAL name :Str)]\nLET Sig = :Named\n\
             EXPR {declared} -> Any = #(\n  EXPR #(KEEP e :{parameter}) -> {parameter} = #(e)\n  \
             LET a = (KEEP x)\n  a\n)\n"
        )
    };
    let tree = |door: &Door<'_, '_>, facts: &LoadFacts<'_>| {
        let f = subject(door.root());
        facts
            .content(f, slot(door.symbols(), f, "a"))
            .map(|tree| tree.identity())
    };
    let under = resumed(&body("FOR ALL #[Elt] #(SUBJECT x :Elt)", "Elt"), |door| {
        let elt = unknown(door.symbols(), subject(door.root()), "Elt");
        let binding = door.binding(&[(elt, door.declared("Sig"))]);
        let resumed = (door.resume(None, binding, false)).expect("refuses nothing");
        tree(door, resumed)
    });
    let fresh = resumed(&body("#(SUBJECT x :Named)", "Named"), |door| {
        tree(door, door.load())
    });
    assert!(fresh.is_some());
    assert_eq!(under, fresh);
}

/// A closure over `Elt` called by name unfolds, and a module viewed at an application over `Elt`
/// is a view at it: each binder's tree under `Elt := Number` is a fresh load's with `Number`
/// written for `Elt`, and the call's member reads `Number` at load already.
#[test]
fn a_call_by_name_and_a_view_over_a_name_are_trees_a_binding_closes() {
    let body = |declared: &str, elt: &str| {
        format!(
            "SIG Coll FOR ALL #[Elem] = #[(VAL first :Elem)]\n\
             EXPR {declared} -> Any = #(\n  \
             LET h = (FN :{{y :{elt}}} -> Module = #(\n    \
             (MODULE built OVER #[y] = (LET Zero = Number))\n    built\n  ))\n  \
             LET a = (h {{y = x}})\n  LET z :(a.Zero) = 5\n  \
             MODULE made OVER #[x] = (LET first = x)\n  \
             LET v = (made :| (Coll WITH {{Elem = {elt}}}))\n  z\n)\n"
        )
    };
    let trees = |door: &Door<'_, '_>, facts: &LoadFacts<'_>| {
        let f = subject(door.root());
        let tree = |name| {
            facts
                .content(f, slot(door.symbols(), f, name))
                .map(|t| t.identity())
        };
        (tree("a"), tree("v"), ends(door, facts, f, "z"))
    };
    let (loaded, under) = resumed(&body("FOR ALL #[Elt] #(SUBJECT x :Elt)", "Elt"), |door| {
        let elt = unknown(door.symbols(), subject(door.root()), "Elt");
        let binding = door.binding(&[(elt, KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("refuses nothing");
        (trees(door, door.load()), trees(door, resumed))
    });
    let fresh = resumed(&body("#(SUBJECT x :Number)", "Number"), |door| {
        trees(door, door.load())
    });
    assert_eq!(loaded.2, "[Number, Number]");
    assert!(fresh.0.is_some() && fresh.1.is_some(), "{fresh:?}");
    assert_eq!(under, fresh);
}

/// A type literal over `Elt` is a type value written over the name, which a binding closes to the
/// type value a fresh load reads: the module built over it is a fresh load's under
/// `Elt := Number`.
#[test]
fn a_type_literal_over_a_name_is_the_type_value_a_binding_closes() {
    let body = |declared: &str, elt: &str| {
        format!(
            "EXPR {declared} -> Any = #(\n  LET t = :(LIST OF {elt})\n  \
             MODULE built OVER #[t] = (LET Zero = Number)\n  LET a = built\n  a\n)\n"
        )
    };
    let tree = |door: &Door<'_, '_>, facts: &LoadFacts<'_>| {
        let f = subject(door.root());
        facts
            .content(f, slot(door.symbols(), f, "a"))
            .map(|tree| tree.identity())
    };
    let under = resumed(&body("FOR ALL #[Elt] #(SUBJECT x :Elt)", "Elt"), |door| {
        let elt = unknown(door.symbols(), subject(door.root()), "Elt");
        let binding = door.binding(&[(elt, KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("refuses nothing");
        tree(door, resumed)
    });
    let fresh = resumed(&body("#(SUBJECT x :Number)", "Number"), |door| {
        tree(door, door.load())
    });
    assert!(
        fresh.is_some_and(|tree| matches!(tree, Identity::Closed(_))),
        "{fresh:?}"
    );
    assert_eq!(under, fresh);
}

/// `LET a = (MKU x)` and `LET b = (MKU x)` call two overloads only a binding of `Elt` selects
/// among: each is the leaf keyed on the call, one tree at load and under `Elt := (Number | Str)`,
/// which selects neither; under `Elt := Number` both unfold to the `Number` overload's module, as
/// a fresh load with `x :Number` reads them.
#[test]
fn two_writes_of_a_call_an_answer_selects_are_one_tree() {
    let body = |declared: &str| {
        format!(
            "LET Either = :(Number | Str)\nEXPR {declared} -> Any = #(\n  \
             EXPR #(MKU n :Number) -> Module = #(\n    \
             (MODULE built OVER #[n] = ((LET Zero = Number) (LET zero = n)))\n    built\n  )\n  \
             EXPR #(MKU s :Str) -> Module = #(\n    \
             (MODULE built OVER #[s] = ((LET Zero = Str) (LET zero = s)))\n    built\n  )\n  \
             LET a = (MKU x)\n  LET b = (MKU x)\n  LET z :(a.Zero) = b.zero\n  z\n)\n"
        )
    };
    let tree = |door: &Door<'_, '_>, facts: &LoadFacts<'_>, name: &str| {
        let f = subject(door.root());
        facts
            .content(f, slot(door.symbols(), f, name))
            .map(|tree| tree.identity())
    };
    let (load, either, number, z) = resumed(&body("FOR ALL #[Elt] #(SUBJECT x :Elt)"), |door| {
        let f = subject(door.root());
        let elt = unknown(door.symbols(), f, "Elt");
        let either = door.binding(&[(elt, door.declared("Either"))]);
        let either = (door.resume(None, either, false)).expect("refuses nothing");
        let number = door.binding(&[(elt, KType::NUMBER)]);
        let number = (door.resume(None, number, false)).expect("refuses nothing");
        let pair = |facts| (tree(door, facts, "a"), tree(door, facts, "b"));
        (
            pair(door.load()),
            pair(either),
            pair(number),
            ends(door, number, f, "z"),
        )
    });
    let (fresh, fresh_z) = resumed(&body("#(SUBJECT x :Number)"), |door| {
        let f = subject(door.root());
        (
            (tree(door, door.load(), "a"), tree(door, door.load(), "b")),
            ends(door, door.load(), f, "z"),
        )
    });
    assert!(
        load.0.is_some_and(|a| matches!(a, Identity::Open(_))),
        "{load:?}"
    );
    assert_eq!(
        load.0, load.1,
        "two writes of one call are one tree at load"
    );
    assert_eq!(either.0, either.1, "and under a binding selecting neither");
    assert_eq!(number, fresh, "a binding selecting one unfolds it");
    assert_eq!(z, fresh_z);
}

/// `Paired`, and a module stamped at it with `Left = Number`.
const PAIRED: &str = "SIG Paired FOR ALL #[Left Right] = #[(VAL left :Left) (VAL right :Right)]\n\
                      MODULE pair = ((LET left = 1) (LET right = \"r\"))\n\
                      LET stamped :(Paired WITH {Left = Number}) = pair\n";

/// `p.Right` reads through `Paired WITH {Left = Elt}`: binding `Elt` alone moves its paths to the
/// class over `Paired WITH {Left = Number}`, which a re-run of every unit names too; binding `p`
/// over that layer closes them to the module's `Right`, as a fresh load reads it.
#[test]
fn binding_a_path_s_application_alone_moves_it_to_the_class_a_fresh_load_names() {
    let body = "#(WRAP x :Elt AND p :(Paired WITH {Left = Elt})) -> Any = #(\n  \
                LET k = p.right\n  LET j :(p.Right) = k\n  j\n)\n";
    let source = format!("{PAIRED}EXPR FOR ALL #[Elt] {body}");
    let (skipping, everything, closed) = resumed(&source, |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let p = unknown(door.symbols(), f, "p");
        let once = door.binding(&[(elt, KType::NUMBER)]);
        let resume = |everything| (door.resume(None, once, everything)).expect("refuses nothing");
        let (skipping, everything) = (resume(false), resume(true));
        let both = door.bound(&[
            (elt, Bound::Type(KType::NUMBER)),
            (p, door.module("stamped")),
        ]);
        let closed = (door.resume(Some(skipping), both, false)).expect("refuses nothing");
        (
            upper(door, skipping, f, "k"),
            upper(door, everything, f, "k"),
            upper(door, closed, f, "k"),
        )
    });
    let restart = format!(
        "{PAIRED}LET Elt = Number\nEXPR #(WRAP x :Elt) -> Any = #(\n  LET p = stamped\n  \
         LET k = p.right\n  LET j :(p.Right) = k\n  j\n)\n"
    );
    let restarted = resumed(&restart, |door| {
        upper(door, door.load(), subject(door.root()), "k")
    });
    assert_eq!(skipping, everything);
    assert_eq!(closed, restarted);
}

/// Two views over two module parameters are two types at the load, and one at the run where the
/// parameters are bound alike: the load refuses nothing reading one's carrier as the other's.
#[test]
fn views_over_two_parameters_refuse_nothing_at_the_load() {
    let source = format!(
        "{COUNTER}MODULE ints = ((LET Carrier = Number) (LET zero = 0) \
         (LET succ = (FN :{{x :Number}} -> Number = #(x))))\n\
         LET stamped :Counter = ints\n\
         EXPR #(OUTER m :Counter AND n :Counter) -> Any = #(\n  LET w = (m :| Counter)\n  \
         LET v = (n :| Counter)\n  LET mixed :(v.Carrier) = w.zero\n  mixed\n)\n\
         PRINT (OUTER stamped AND ints)\n"
    );
    assert_eq!(super::run(&source), "Carrier(0)");
}

/// `Picks`, a signature over a `Carrier` whose `PICK` head a `USING` surfaces.
const PICKS: &str = "SIG Picks FOR ALL #[Carrier] = \
                     #[(VAL zero :Carrier) (EXPR #(PICK x :Carrier) -> Str)]\n";

/// The unit binding `name` in `shape`.
pub(super) fn unit_of(
    door: &Door<'_, '_>,
    shape: &BodyShape<'_>,
    name: &str,
) -> crate::scope::UnitId {
    crate::scope::UnitId {
        segment: shape.segment(),
        unit: shape.unit_of_slot(slot(door.symbols(), shape, name)),
    }
}

/// How the load's record of `unit` waits on `unknown`: `None` where it does not, else whether any
/// mark on it is open, beside the kinds of those marks.
fn waits_on(
    door: &Door<'_, '_>,
    unit: crate::scope::UnitId,
    unknown: Unknown,
) -> Option<Vec<Option<crate::scope::OpenKind>>> {
    let (_, frontier) = door.load().unit(unit).expect("the load ran the unit");
    let marks: Vec<_> = (frontier.marks().into_iter())
        .filter(|noted| noted.unknown == unknown)
        .map(|noted| match noted.mark {
            crate::scope::Mark::Named => None,
            crate::scope::Mark::Open(open) => Some(open.kind),
        })
        .collect();
    (!marks.is_empty()).then_some(marks)
}

/// Whether `resumed` re-ran `unit`.
fn reran(resumed: &LoadFacts<'_>, unit: crate::scope::UnitId) -> bool {
    (resumed.own_units()).any(|(held, _)| held == unit)
}

/// A unit over a list of a bound name only names it: a binding absorbs its record, reads it with
/// the type written in, and lays the list it closes to in the layer's table of closed types.
#[test]
fn a_unit_over_a_list_of_its_name_is_read_closed_through_its_layers_table() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP xs :(LIST OF Elt)) -> Any = #(\n  \
                  LET ys = xs\n  ys\n)";
    let (named, absorbed, typed, tabled) = resumed(source, |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let ys = unit_of(door, f, "ys");
        let named = waits_on(door, ys, elt);
        let binding = door.binding(&[(elt, KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let tabled: Vec<String> = (resumed.base_layer().closed().entries())
            .map(|(_, value)| match value {
                crate::scope::Closed::Type(closed) => {
                    display_name(closed, door.types, door.symbols()).to_string()
                }
                other => format!("{other:?}"),
            })
            .collect();
        (
            named,
            !reran(resumed, ys),
            upper(door, resumed, f, "ys"),
            tabled,
        )
    });
    assert_eq!(
        named,
        Some(vec![None]),
        "`ys` names `Elt` and judges nothing over it"
    );
    assert!(absorbed, "a binding of `Elt` absorbs `ys`'s record");
    assert_eq!(
        typed, ":(LIST OF Number)",
        "and reads it with the type written in"
    );
    assert!(
        tabled.contains(&":(LIST OF Number)".to_string()),
        "{tabled:?}"
    );
}

/// A reader of a bare-variable parameter holds on no retype: the parameter's load-time type is
/// `[(RETYPED TO Elt), Elt]`, whose lower end a binding gives the retype's own, so `Elt := Number`
/// makes it exactly `Number` by substitution. Only a discrete question marks the reader.
#[test]
fn a_bare_parameters_reader_commutes_by_its_retype() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET y = x\n  y\n)";
    let (marks, typed) = resumed(source, |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let y = unit_of(door, f, "y");
        let marks = waits_on(door, y, elt).unwrap_or_default();
        let binding = door.binding(&[(elt, KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        (marks, upper(door, resumed, f, "y"))
    });
    assert!(
        !marks
            .iter()
            .any(|mark| mark.is_some_and(|kind| kind.holds())),
        "{marks:?}"
    );
    assert_eq!(typed, "Number");
}

/// A binder's reader reaches its seed alone: a reader of `xs` names `Elt` and waits on nothing of
/// `Key`, and a value read of `Key` names it and asks nothing. A type naming
/// `Key` reads no seed: `zs`'s annotation is open only by its retype.
#[test]
fn a_binders_reader_reaches_its_seed_alone() {
    let source = "EXPR FOR ALL #[Elt Key] #(WRAP xs :(LIST OF Elt) AND k :Key) -> Any = #(\n  \
                  LET ys = xs\n  LET t = Key\n  LET zs :(LIST OF Key) = []\n  ys\n)";
    let (ys, t, zs, rerun) = resumed(source, |door| {
        let f = quantified(door);
        let (elt, key) = (
            unknown(door.symbols(), f, "Elt"),
            unknown(door.symbols(), f, "Key"),
        );
        let (ys, t, zs) = (
            unit_of(door, f, "ys"),
            unit_of(door, f, "t"),
            unit_of(door, f, "zs"),
        );
        let waits = (
            (waits_on(door, ys, elt), waits_on(door, ys, key)),
            waits_on(door, t, key),
            waits_on(door, zs, key),
        );
        let binding = door.binding(&[(key, KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        (
            waits.0,
            waits.1,
            waits.2,
            (reran(resumed, ys), reran(resumed, t), reran(resumed, zs)),
        )
    });
    assert_eq!(ys, (Some(vec![None]), None), "`ys` names `Elt` alone");
    assert_eq!(
        t,
        Some(vec![None]),
        "`t` reads `Key`'s value, whose type asks nothing: `Key`'s bound is no signature, so `t` \
         is no module binder"
    );
    assert_eq!(
        zs,
        Some(vec![Some(crate::scope::OpenKind::Retype)]),
        "`zs` reaches no seed of `Key`: its marks are its own, its retype alone, since `[]` is no \
         module the annotation could check"
    );
    assert_eq!(
        rerun,
        (false, false, true),
        "a binding of `Key` re-runs `zs` alone"
    );
}

/// An absorbed unit's call sites carry the requests a walked layer lays: a layer's requests come
/// from the load's slot maps and the ask, not from re-run records (F7).
#[test]
fn an_absorbed_units_requests_are_a_walked_layers() {
    let source = format!(
        "{COUNTER}MODULE ints = ((LET zero = 0) (LET succ = (FN :{{x :Number}} -> Number = #(x + 1))))\n\
         EXPR #(CARRIER m :Counter) -> Number = #((LET v = (m :| Counter)) (7))\n\
         EXPR FOR ALL #[Elt] #(WRAP xs :(LIST OF Elt) AND m :Counter) -> Any = #(\n  \
         LET ys = xs\n  LET n = (CARRIER m)\n  ys\n)\n"
    );
    resumed(&source, |door| {
        let f = quantified(door);
        let binding = door.bound(&[
            (
                unknown(door.symbols(), f, "Elt"),
                Bound::Type(KType::NUMBER),
            ),
            (unknown(door.symbols(), f, "m"), door.module("ints")),
        ]);
        let skipping = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let walked = (door.resume(None, binding, true)).expect("the resumption refuses nothing");
        assert!(!reran(skipping, unit_of(door, f, "ys")), "`ys` is absorbed");
        let sites: Vec<_> = door
            .load()
            .traces(f)
            .iter()
            .map(|(site, _)| *site)
            .collect();
        assert!(!sites.is_empty(), "`WRAP` traces `(CARRIER m)`");
        for site in sites {
            let request = |facts: &LoadFacts<'_>| {
                (facts.base_layer().request(f.segment(), site)).map(|each| each.key)
            };
            assert_eq!(request(skipping), request(walked), "at {site:?}");
        }
    });
}

/// A scheme over two names two layers bind: `h`'s type names `Elt`, which `f`'s frame binds, and
/// `Key`, which `g`'s frame binds beneath it. Each layer writes in the level its binding binds and
/// keeps the other, so no rigid fact reaches the run.
#[test]
fn a_scheme_over_two_layers_names_reaches_the_run_closed() {
    let source = "MODULE lib = (LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Any = #(\n  \
                  MODULE mid OVER #[Elt x] = (LET g = (FN FOR ALL #[Key] :{k :Key} -> Any = #(\n    \
                  MODULE inner OVER #[Elt Key] = \
                  (LET h = (FN FOR ALL #[Tee] :{t :Tee, u :Elt, v :Key} -> Tee = #(t)))\n    \
                  (inner.h {t = 1, u = x, v = k})\n  )))\n  \
                  (mid.g {k = \"s\"})\n)))\n\
                  PRINT (lib.f {x = 2})\nPRINT (lib.f {x = \"w\"})\n";
    crate::scope::RIGID_AT_RUN.with(|count| count.set(0));
    assert_eq!(super::run(source), "1\n1");
    assert_eq!(crate::scope::RIGID_AT_RUN.with(std::cell::Cell::get), 0);
}

/// The laws over drawn [`Resumable`] programs, each fact rendered by names — never by an index or
/// a source position — so a resumption and a restart render alike where they agree.
pub(super) mod laws {
    use proptest::prelude::*;

    use crate::memory::Bump;
    use crate::parse::ExpressionPart;
    use crate::program::tests::stores::framed;
    use crate::program::{CellSubstrate, LoadError};
    use crate::scope::{
        Address, BodyClass, BodyShape, Bound as Bind, CandidateList, FactsOver, InstanceRead,
        LoadFacts, Narrowing, RecordSource, ShapeError, ShapeKind, Site, Slot, Static,
        StaticInstance, Target, Tree, Unknown, each_part, source_of,
    };
    use crate::symbols::{BinderSymbol, SymbolInterner};
    use crate::type_lattice::{
        DeclaredType, Interval, KType, Parametric, Variable, Verdict, display_name, fits,
    };
    use crate::values::tests::reading;
    use crate::values::{BuildBudget, Chain};

    use super::super::generate::{
        ALTERNATED, Bound, Resumable, Witnessable, cellular, resumable, witnessable,
    };
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

    /// [`binding`], each module bound to its content tree's digest beside the module as a view
    /// over it reads it, as a frame binds the module its slot holds where `by_digest`: a `MODULE`
    /// body is a transparent view of itself at the signature it carries.
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
                matches!(name, "m" | "n").then(|| door.module(&Resumable::stamped(name))),
            ) {
                (_, Some(Bind::Module { carried, tree })) if by_digest => {
                    // The module's own content beside the module as a view over it reads it, a
                    // `MODULE` body a transparent view of itself, retyped as a frame stamps it.
                    let (own, source) = match tree {
                        Tree::Retyped { source, .. } => {
                            let Bind::Module { carried: own, .. } = door.module(&declared) else {
                                unreachable!("a module binds a module")
                            };
                            (own, **source)
                        }
                        _ => (carried, *tree),
                    };
                    let bare = matches!(source, Tree::Body { .. }).then(|| {
                        crate::memory::resident(
                            door.bump,
                            crate::scope::Bare {
                                signature: Some(own),
                                sealing: None,
                            },
                        )
                    });
                    let digest = Tree::Digest(source.digest().expect("a closed tree"), bare);
                    let tree = match tree {
                        Tree::Retyped { .. } => Tree::retyped_closed(
                            carried,
                            crate::memory::resident(door.bump, digest),
                        ),
                        _ => digest,
                    };
                    Bind::Module {
                        carried,
                        tree: crate::memory::resident(door.bump, tree),
                    }
                }
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
            Narrowing::Kept(kept) => {
                kept.entries
                    .iter()
                    .fold("kept".to_string(), |text, (at, verdict)| {
                        let at = match at {
                            crate::scope::Candidate::One(coordinate) => index(*coordinate),
                            crate::scope::Candidate::Spread(_) => usize::MAX,
                        };
                        format!("{text} {at}:{verdict:?}")
                    })
            }
            Narrowing::Selected(coordinate) => format!("selected {}", index(coordinate)),
        }
    }

    /// What the load fixed of a facts' reader: the chain its types render through.
    struct Reader<'read, 'tier, 'pair, 'reg, 'door, S> {
        facts: &'read FactsOver<S>,
        types: &'read Chain<'tier, 'pair, 'reg, 'door>,
        symbols: &'read SymbolInterner,
        /// The names a binding binds: a restart drops them from the shape or binds them anew, so
        /// what the load fixed for each binder itself is no fact a restart has.
        bound: &'read [&'read str],
        /// Law 14's reading: only the lines a unit not open holds, each type with the binding's
        /// levels written in. `None` renders every line as the facts hold it.
        substituting: Option<&'read Substituting>,
        /// Whether every type the rigid fact being rendered names closed under substitution.
        closes: std::cell::Cell<bool>,
    }

    /// Where a rendered line rests: the unit whose record holds its fact.
    #[derive(Clone, Copy)]
    enum Line {
        Slot(crate::scope::Segment, Slot),
        Site(crate::scope::Segment, Site),
        Statement(crate::scope::Segment, usize),
        Callable(crate::scope::Segment),
    }

    /// Law 14's reading of a load's facts: the levels a binding binds, the unit holding each site
    /// and each born body, and the units whose frontier meets an open mark on a bound name.
    struct Substituting {
        levels: Vec<(usize, KType)>,
        sites: std::collections::HashMap<(crate::scope::Segment, Site), crate::scope::UnitId>,
        born: std::collections::HashMap<crate::scope::Segment, crate::scope::UnitId>,
        open: Vec<crate::scope::UnitId>,
    }

    impl<'f, S: RecordSource<'f>> Reader<'_, '_, '_, '_, '_, S> {
        fn named(&self, typed: impl Into<DeclaredType<Parametric>>) -> String {
            let typed = self.read(typed.into());
            display_name(typed, self.types, self.symbols).to_string()
        }

        /// `typed` as this render reads it: with the binding's levels written in under law 14,
        /// each derived name its term handed again with them written in.
        fn read(&self, typed: DeclaredType<Parametric>) -> DeclaredType<Parametric> {
            let Some(substituting) = self.substituting else {
                return typed;
            };
            let scratch = Bump::new();
            let levels = crate::scope::Levels::from_bound(&scratch, &substituting.levels);
            let names = |id| self.facts.derived_name(id);
            let binds = crate::scope::LevelsOver {
                levels: &levels,
                names: &names,
                spellings: crate::scope::Spellings::new(self.symbols),
                laid: &|_| {},
            };
            let read = match crate::scope::rebound(self.types, &scratch, typed, &binds) {
                crate::scope::Closed::Same => typed,
                crate::scope::Closed::Type(closed) => closed.into(),
                crate::scope::Closed::Partial(partial) => partial,
            };
            let rigid = match read {
                DeclaredType::Type(typed) => self.types.concrete(typed).is_none(),
                DeclaredType::Scheme(_) => {
                    !crate::elaborate::judge::free_variables(self.types, &scratch, read).is_empty()
                }
            };
            if rigid {
                self.closes.set(false);
            }
            read
        }

        /// `typed`'s ends, rendered as this render reads them.
        fn interval(&self, typed: Interval) -> String {
            let read = |end: Parametric| match self.read(end.into()) {
                DeclaredType::Type(end) => end,
                DeclaredType::Scheme(_) => unreachable!("an end is a type"),
            };
            interval(
                Interval {
                    lower: read(typed.lower),
                    upper: read(typed.upper),
                },
                self.types,
                self.symbols,
            )
        }

        /// A load-time type, by its names: a rigid one law 14 closes whole reads as closed, as the
        /// restart fixes it.
        fn statically<C, R>(
            &self,
            typed: Static<'_, C, R>,
            closed: impl Fn(C) -> String,
            rigid: impl Fn(R) -> String,
        ) -> String {
            match typed {
                Static::Rigid { value, .. } if self.substituting.is_some() => {
                    self.closes.set(true);
                    let named = rigid(value);
                    match self.closes.get() {
                        true => format!("closed {named}"),
                        false => format!("rigid {named}"),
                    }
                }
                typed => statically(typed, closed, rigid),
            }
        }

        /// Whether `line` is rendered: always, or under law 14 where a unit not open holds it.
        fn keeps(&self, line: Line) -> bool {
            let Some(substituting) = self.substituting else {
                return true;
            };
            let holder = match line {
                Line::Slot(segment, slot) => Some(crate::scope::UnitId {
                    segment,
                    unit: self.facts.shapes().shape_at(segment).unit_of_slot(slot),
                }),
                Line::Statement(segment, index) => Some(crate::scope::UnitId {
                    segment,
                    unit: self
                        .facts
                        .shapes()
                        .shape_at(segment)
                        .unit_of_statement(index),
                }),
                Line::Site(segment, site) => substituting.sites.get(&(segment, site)).copied(),
                Line::Callable(segment) => substituting.born.get(&segment).copied(),
            };
            holder.is_some_and(|unit| !substituting.open.contains(&unit))
        }

        fn solution(&self, solution: StaticInstance<'_>) -> String {
            match solution {
                StaticInstance::Closed { solution, .. } => {
                    let points: Vec<String> =
                        solution.iter().map(|point| self.named(*point)).collect();
                    format!("closed {points:?}")
                }
                StaticInstance::Rigid { solution, .. } => {
                    self.closes.set(true);
                    let points: Vec<String> =
                        solution.iter().map(|point| self.named(*point)).collect();
                    match self.substituting.is_some() && self.closes.get() {
                        true => format!("closed {points:?}"),
                        false => format!("rigid {points:?}"),
                    }
                }
            }
        }

        /// A selection's point, by name: a closed one, else `open`. Law 14 reads a rigid one with
        /// the binding written in, and names it where that closes it.
        fn point(&self, point: crate::scope::Point) -> String {
            match point {
                crate::scope::Point::Closed(point) => self.named(point),
                crate::scope::Point::Rigid(point) if self.substituting.is_some() => {
                    self.closes.set(true);
                    let named = self.named(point);
                    match self.closes.get() {
                        true => named,
                        false => "open".to_string(),
                    }
                }
                crate::scope::Point::Rigid(_) | crate::scope::Point::Carried(_) => {
                    "open".to_string()
                }
            }
        }

        /// Every fact of `shape` and the shapes nested in it, rendered by names under `path`.
        fn render(&self, shape: &BodyShape<'_>, path: &str, out: &mut Vec<String>) {
            let (facts, symbols) = (self.facts, self.symbols);
            let segment = shape.segment();
            // Binders by name: a restart numbers its slots apart.
            let mut binders = Vec::new();
            for at in (0..shape.slots()).map(|at| Slot(at as u32)) {
                let named = shape.slot_name(at);
                let spelled = symbols.display(named.symbol()).to_string();
                // A bound name's slot is left out: `SUBJECT`'s at the top path, and a cell's `Cel`
                // in the cell, whose restart has no such slot.
                if (path.is_empty() || spelled == "Cel") && self.bound.contains(&spelled.as_str()) {
                    continue;
                }
                let name = format!("{path} {spelled}");
                match named {
                    BinderSymbol::Value(_) => {
                        let typed = match facts.binder_type(self.types, shape, at) {
                            Some(DeclaredType::Type(typed)) => self.interval(typed),
                            Some(DeclaredType::Scheme(scheme)) => self.named(scheme),
                            None => "untyped".to_string(),
                        };
                        if self.keeps(Line::Slot(segment, at)) {
                            binders.push(format!("{name} = {typed}"));
                        }
                    }
                    BinderSymbol::Type(_) => {
                        let typed = facts.declared_type(self.types, shape, at);
                        let typed = self.statically(
                            typed,
                            |closed| self.named(closed),
                            |rigid| self.named(rigid),
                        );
                        if self.keeps(Line::Slot(segment, at)) {
                            binders.push(format!("{name} declares {typed}"));
                        }
                    }
                    // A code shape's hole or mark is a capture, and holds no fact of its own.
                    BinderSymbol::Key(_) => {}
                    BinderSymbol::Registration(_) => {
                        let typed = facts.registered_type(self.types, shape, at);
                        let typed = self.statically(
                            typed,
                            |closed| self.named(closed.shape),
                            |rigid| self.named(rigid.shape),
                        );
                        if self.keeps(Line::Slot(segment, at)) {
                            binders.push(format!("{name} registers {typed}"));
                        }
                    }
                }
                if let Some(tree) = facts.content(shape, at)
                    && self.keeps(Line::Slot(segment, at))
                {
                    binders.push(format!("{name} holds a tree {}", super::tree_line(&tree)));
                }
            }
            binders.sort();
            out.extend(binders);
            // `SUBJECT`'s own type is a fact of the unit birthing it.
            if shape.kind() == ShapeKind::Callable && !path.is_empty() {
                let typed = facts.callable_type(self.types, shape);
                let typed = self.statically(
                    typed,
                    |closed| self.named(closed.ktype),
                    |rigid| self.named(rigid.ktype),
                );
                if self.keeps(Line::Callable(segment)) {
                    out.push(format!("{path} callable {typed}"));
                }
            }
            let mut expressions: Vec<_> = shape.type_expressions().iter().collect();
            expressions.sort_by_key(|expression| expression.site);
            for (index, expression) in expressions.into_iter().enumerate() {
                let typed = facts.typed_expression(self.types, shape, expression.site);
                let typed = self.statically(
                    typed,
                    |closed| self.named(closed),
                    |rigid| self.named(rigid),
                );
                if self.keeps(Line::Site(segment, expression.site)) {
                    out.push(format!("{path} type {index} = {typed}"));
                }
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
                let typed = facts.statement_type(self.types, shape, index);
                let typed = typed.map_or("untyped".to_string(), |typed| self.interval(typed));
                if self.keeps(Line::Statement(segment, index)) {
                    out.push(format!("{path} #{written} = {typed}"));
                }
                let mut sites = Vec::new();
                each_part(statement, |part| sites.push(shape.site(part)));
                let mut at = 0;
                for site in sites.iter() {
                    let Some(typed) = facts.value_type(self.types, shape, *site) else {
                        continue;
                    };
                    let part = format!("{path} #{written}.{at}");
                    at += 1;
                    let line = Line::Site(segment, *site);
                    let typed = self.interval(typed);
                    if self.keeps(line) {
                        out.push(format!("{part} = {typed}"));
                    }
                    match facts.instance_at(self.types, &crate::memory::Bump::new(), shape, *site) {
                        Some(InstanceRead::Solved(solution)) => {
                            let solution = self.solution(solution);
                            if self.keeps(line) {
                                out.push(format!("{part} instance {solution}"));
                            }
                        }
                        Some(InstanceRead::AsIs) if self.keeps(line) => {
                            out.push(format!("{part} instance as is"));
                        }
                        _ => {}
                    }
                    let closing = crate::memory::Bump::new();
                    let named = facts.named_contributions(self.types, &closing, shape, *site);
                    if !named.is_empty() {
                        let named: Vec<String> = (named.iter())
                            .map(|typed| {
                                self.statically(
                                    *typed,
                                    |closed| self.named(closed),
                                    |rigid| self.named(rigid),
                                )
                            })
                            .collect();
                        if self.keeps(line) {
                            out.push(format!("{part} by name {named:?}"));
                        }
                    }
                    if let Some(tree) = facts.view_tree(shape, *site)
                        && self.keeps(line)
                    {
                        out.push(format!("{part} views a tree {}", super::tree_line(tree)));
                    }
                }
                let mut uses = vec![shape.site_of_node(statement)];
                each_part(statement, |part| {
                    if let ExpressionPart::Expression(node) = part {
                        uses.push(shape.site_of_node(node));
                    }
                });
                for (at, site) in uses.into_iter().enumerate() {
                    let Some((_, list)) = shape.candidate_lists().get(site) else {
                        continue;
                    };
                    let closing = crate::memory::Bump::new();
                    let fixed = facts.use_statics(self.types, &closing, shape, site);
                    let contributions: Vec<String> = (fixed.contributions.iter())
                        .map(|typed| {
                            self.statically(
                                *typed,
                                |closed| self.named(closed),
                                |rigid| self.named(rigid),
                            )
                        })
                        .collect();
                    if self.keeps(Line::Site(segment, site)) {
                        out.push(format!(
                            "{path} #{written} use {at}: {} from {contributions:?}",
                            narrowed(fixed.narrowing, list)
                        ));
                    }
                    if let Some(points) =
                        facts.selection(self.types, &crate::memory::Bump::new(), shape, site)
                    {
                        let points: Vec<String> = (points.points.iter())
                            .map(|point| self.point(*point))
                            .collect();
                        if self.keeps(Line::Site(segment, site)) {
                            out.push(format!("{path} #{written} use {at} solves {points:?}"));
                        }
                    }
                }
                let settled = (sites.iter())
                    .filter(|site| facts.settled(shape, **site))
                    .count();
                let held = (sites.iter()).all(|site| self.keeps(Line::Site(segment, *site)));
                if held && self.keeps(Line::Statement(segment, index)) {
                    out.push(format!("{path} #{written} settled {settled}"));
                }
            }
            for (index, (_, nested)) in shape.nested_shapes().iter().enumerate() {
                self.render(nested, &format!("{path}/{index}"), out);
            }
        }
    }

    /// The lines of `SUBJECT`'s render of the load that units not open over `bound` hold, each type
    /// read with `bound`'s levels written in: law 14's side of the comparison.
    fn commuted(door: &Door<'_, '_>, bound: &Bound) -> Vec<String> {
        let binding = binding(door, bound);
        let load = door.load();
        let mut substituting = Substituting {
            levels: levels(door, bound),
            sites: std::collections::HashMap::new(),
            born: std::collections::HashMap::new(),
            open: Vec::new(),
        };
        // Law 14 writes in a binding's type levels alone: a record naming a module binder the
        // binding binds, or holding a tree written over names, closes by module and tree
        // substitution, which the restart laws check.
        let module_named = |frontier: crate::scope::Frontier<'_>| {
            (frontier.marks().iter())
                .any(|noted| matches!(binding.bound(noted.unknown), Some(Bind::Module { .. })))
        };
        for (unit, (facts, frontier)) in load.own_units() {
            if frontier.meets_open(binding)
                || module_named(frontier)
                || super::writes_tree_names(door.types, facts)
            {
                substituting.open.push(unit);
            }
            for (segment, typed) in facts.typed.iter() {
                typed.held(*segment, &mut |held| {
                    if let crate::scope::Held::Site(segment, site) = held {
                        substituting.sites.insert((segment, site), unit);
                    }
                });
            }
            for (segment, _) in facts.born.iter() {
                substituting.born.insert(*segment, unit);
            }
        }
        let names = left_out(bound);
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let reader = Reader {
            facts: load,
            types: door.types,
            symbols: door.symbols(),
            bound: &names,
            substituting: Some(&substituting),
            closes: std::cell::Cell::new(true),
        };
        let mut out = Vec::new();
        reader.render(subject(load.shapes().root()), "", &mut out);
        out.iter().map(|line| canonical(line)).collect()
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
        render_over(facts, types, symbols, leave)
    }

    /// [`render_facts`] over any source of records.
    pub(in super::super) fn render_over<'f, S: RecordSource<'f>>(
        facts: &FactsOver<S>,
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
            substituting: None,
            closes: std::cell::Cell::new(true),
        };
        let mut out = Vec::new();
        reader.render(subject(facts.shapes().root()), "", &mut out);
        out.iter().map(|line| canonical(line)).collect()
    }

    /// A refusal's message, its location dropped.
    fn unlocated(message: &str) -> String {
        canonical(match message.split_once(": ") {
            Some((at, rest)) if at.starts_with('<') => rest,
            _ => message,
        })
    }

    /// `line` with each record type's fields, `:{x :Number y :Str}`, in name order, and each
    /// union's members, `(Number | Str)`, in text order: a record's and a union's identity is
    /// blind to their order, which a rendering takes from the spelling that made the type first,
    /// and two programs may make one type first from different spellings.
    pub(in super::super) fn canonical(line: &str) -> String {
        let chars: Vec<char> = line.chars().collect();
        let mut at = 0;
        canonical_until(&chars, &mut at, None)
    }

    /// The canonical text from `at` up to `close`, or the end, `at` left past it.
    fn canonical_until(chars: &[char], at: &mut usize, close: Option<char>) -> String {
        let mut out = String::new();
        while *at < chars.len() {
            let c = chars[*at];
            *at += 1;
            if Some(c) == close {
                return out;
            }
            match c {
                '{' if out.ends_with(':') => {
                    let inner = canonical_until(chars, at, Some('}'));
                    out.push('{');
                    out.push_str(&sorted_fields(&inner));
                    out.push('}');
                }
                '(' => {
                    let inner = canonical_until(chars, at, Some(')'));
                    out.push('(');
                    out.push_str(&sorted_members(&inner));
                    out.push(')');
                }
                '{' | '[' => {
                    let closing = match c {
                        '{' => '}',
                        _ => ']',
                    };
                    out.push(c);
                    out.push_str(&canonical_until(chars, at, Some(closing)));
                    out.push(closing);
                }
                _ => out.push(c),
            }
        }
        out
    }

    /// A parenthesized group's text, its members in order where it is a union, `Number | Str`: its
    /// parts split at its own depth.
    fn sorted_members(inner: &str) -> String {
        let mut members: Vec<String> = Vec::new();
        let mut depth = 0i32;
        let mut member = String::new();
        let chars: Vec<char> = inner.chars().collect();
        let mut at = 0;
        while at < chars.len() {
            let c = chars[at];
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ => {}
            }
            if depth == 0 && chars[at..].starts_with(&[' ', '|', ' ']) {
                members.push(std::mem::take(&mut member));
                at += 3;
                continue;
            }
            member.push(c);
            at += 1;
        }
        members.push(member);
        if members.len() > 1 {
            members.sort();
        }
        members.join(" | ")
    }

    /// A record's fields, `x :Number y :Str`, in name order: each a name, then its type's
    /// tokens, which open with `:` at the record's own depth.
    fn sorted_fields(inner: &str) -> String {
        let mut fields: Vec<String> = Vec::new();
        let mut depth = 0i32;
        let mut token = String::new();
        let mut tokens = Vec::new();
        for c in inner.chars() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ => {}
            }
            if c == ' ' && depth == 0 {
                tokens.push(std::mem::take(&mut token));
            } else {
                token.push(c);
            }
        }
        tokens.push(token);
        for token in tokens.into_iter().filter(|token| !token.is_empty()) {
            match fields.last_mut() {
                Some(field) if token.starts_with(':') => {
                    field.push(' ');
                    field.push_str(&token);
                }
                _ => fields.push(token),
            }
        }
        fields.sort();
        fields.join(" ")
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

    /// What `bound` asks a miss to settle at `SUBJECT`'s body: each type it binds, and each module
    /// at the exact type it carries over the digest of the tree the load read for it.
    fn ask(door: &Door<'_, '_>, bound: &Bound) -> crate::scheduler::Ask {
        let binding = bind(door, bound, false);
        let asked = (binding.entries().iter()).map(|(unknown, given)| {
            let Address::Slot(slot) = unknown.at else {
                unreachable!("a binder's unknown is its slot")
            };
            match *given {
                Bind::Type(named) => crate::scheduler::Asked::Type { slot, named },
                Bind::Module { carried, tree } => {
                    // A frame asks for the module's own content, retyped where it stamps it.
                    let (content, retyped) = match tree {
                        Tree::Retyped { source, .. } => (*source, true),
                        _ => (tree, false),
                    };
                    crate::scheduler::Asked::Module {
                        slot,
                        composition: None,
                        carried,
                        content: content.digest().expect("a module binds a closed tree"),
                        retyped,
                    }
                }
            }
        });
        crate::scheduler::Ask {
            body: subject(door.root()).segment(),
            bound: asked.collect(),
        }
    }

    /// What settling `case`'s `SUBJECT` under its binding renders, as a frame under the
    /// settlement reads it, or the refusal it keeps.
    fn settled_rendered(case: &Resumable) -> Result<Vec<String>, String> {
        resumed(&case.render(), |door| {
            let facts = (door.running)
                .settle::<Koan>(&ask(door, &case.bound), door.bump)
                .expect("every module the case binds has a tree in the load");
            match facts.refusal(door.types) {
                Some(refusal) => Err(unlocated(
                    &refusal.display(door.symbols(), door.types).to_string(),
                )),
                None => Ok(render_over(
                    &facts,
                    door.types,
                    door.symbols(),
                    &left_out(&case.bound),
                )),
            }
        })
    }

    /// A layer the load laid equals the settlement a miss lays
    /// for the same request beneath the same parent. Over each of the load's layer fixtures (a
    /// view, a retype, a call by name, `g` under `OUTER`, `CARRIER` under `PASS`, and more), every
    /// layer in the load's table renders its callee's body as [`Running::settle_under`] of the
    /// request naming it does, and carries the same requests, entry for entry.
    ///
    /// [`Running::settle_under`]: crate::program::Running::settle_under
    #[test]
    fn a_layer_the_load_laid_equals_the_settlement_a_miss_lays() {
        use crate::scope::{CallTrace, Facts};
        let mut checked = Vec::new();
        for (name, source) in super::super::laid::fixtures() {
            let mut each = resumed(&source, |door| {
                let program = door.running.program();
                let (shapes, load) = (program.shapes(), door.load());
                // Every named site, beside its call's text.
                let mut sites = Vec::new();
                for shape in shapes.bodies() {
                    for (site, trace) in load.traces(shape).iter() {
                        if let CallTrace::Named { .. } = trace {
                            let call = super::super::trace::call_of(shape, *site);
                            sites.push((*shape, *site, call.map(|call| call.text())));
                        }
                    }
                }
                let mut checked = Vec::new();
                for laid in load.layers() {
                    // The base's requests, then each laid layer's.
                    let requests = (sites.iter()).flat_map(|(shape, site, call)| {
                        let laid = (load.layers().iter())
                            .filter_map(|each| each.layer.request(shape.segment(), *site));
                        (load.request(shape, *site).into_iter().chain(laid))
                            .map(|request| (request, call.clone()))
                    });
                    let (request, call) = (requests.into_iter())
                        .find(|(request, _)| request.key == laid.key)
                        .unwrap_or_else(|| panic!("{name}: a request names each laid layer"));
                    let sought = request.sought();
                    let settled = (door.running)
                        .settle_under::<Koan>(laid.parent, &sought.ask, door.bump)
                        .expect("every module a request binds has a tree in the load");
                    let body = shapes.shape_at(request.body);
                    let render = |facts: &Facts<'_>| {
                        let reader = Reader {
                            facts,
                            types: door.types,
                            symbols: door.symbols(),
                            bound: &[],
                            substituting: None,
                            closes: std::cell::Cell::new(true),
                        };
                        let mut out = Vec::new();
                        reader.render(body, "", &mut out);
                        out
                    };
                    let at_load = Facts::under(load, Some(laid.layer));
                    assert!(settled.refusal(door.types).is_none(), "{name}: {call:?}");
                    assert_eq!(render(&at_load), render(&settled), "{name}: {call:?}");
                    let top = settled.top().expect("a settlement is its facts' top layer");
                    for (shape, site, at) in &sites {
                        let requested = |layer: &crate::scope::BuildLayer<'_>| {
                            layer
                                .request(shape.segment(), *site)
                                .map(|each| each.sought())
                        };
                        assert_eq!(
                            requested(laid.layer),
                            requested(top),
                            "{name}: {call:?}'s layer, the request at {at:?}"
                        );
                    }
                    checked.push(call.unwrap_or_default());
                }
                checked
            });
            checked.append(&mut each);
        }
        for call in [
            "(CARRIER (ints :| Counter))",
            "(CARRIER (ints :! Counter))",
            "(peek {k = one})",
            "(g {k = one})",
            "(CARRIER m)",
        ] {
            assert!(
                checked.iter().any(|each| each == call),
                "{call}: {checked:?}"
            );
        }
    }

    /// What `SUBJECT`'s frame decides in its activation under the binding, a module bound by its
    /// digest or by its tree, rendered as [`settled_rendered`] renders a settlement, or its
    /// refusal.
    fn activated_rendered(case: &Resumable, by_digest: bool) -> Result<Vec<String>, String> {
        resumed(&case.render(), |door| {
            let own = bind(door, &case.bound, by_digest);
            let at = subject(door.root()).segment();
            match (door.running).activate::<Koan>(door.types, own, at, door.bump) {
                Ok(facts) => Ok(render_over(
                    &facts,
                    door.types,
                    door.symbols(),
                    &left_out(&case.bound),
                )),
                Err(refusal) => Err(unlocated(
                    &refusal.display(door.symbols(), door.types).to_string(),
                )),
            }
        })
    }

    /// What a plain load of `source` renders, or its refusal, but of the binders `bound` names.
    fn loaded_rendered(source: &str, bound: &Bound) -> Result<Vec<String>, String> {
        let loaded =
            CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output());
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

    thread_local! {
        /// Each unit a rooted resumption visited, in order.
        static VISITED: std::cell::RefCell<Vec<crate::scope::UnitId>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Note that a walk visited `unit`.
    fn visited(unit: crate::scope::UnitId) {
        VISITED.with(|visited| visited.borrow_mut().push(unit));
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

        /// Naming commutes with binding: under the binding, each derived name the load laid
        /// gives what its term, with the binding written in, hands back afresh — a closed type, a
        /// spelling, or the name of the term left open, one handle wherever a table holds it.
        #[test]
        fn naming_commutes_with_binding(case in resumable()) {
            let broken = resumed(&case.render(), |door| {
                let shapes = door.running.program().shapes();
                let levels = crate::scope::Levels::of(door.bump, shapes, binding(door, &case.bound));
                let Ok(resumed) = resume_all(door, std::slice::from_ref(&case.bound), false) else {
                    return Vec::new();
                };
                let names = |id| resumed.derived_name(id).or_else(|| door.load().derived_name(id));
                let binds = crate::scope::LevelsOver {
                    levels: &levels,
                    names: &names,
                    spellings: crate::scope::Spellings::new(door.symbols()),
                    laid: &|_| {},
                };
                let mut broken = Vec::new();
                for name in door.load().base_layer().derived().names() {
                    let image = match crate::scope::rebound(door.types, door.bump, name.ty.into(), &binds) {
                        crate::scope::Closed::Same => continue,
                        crate::scope::Closed::Type(closed) => DeclaredType::from(closed),
                        crate::scope::Closed::Partial(partial) => partial,
                    };
                    let (term, _) = name.term.substituted(door.types, door.bump, &binds);
                    let spellings = crate::scope::Spellings::new(door.symbols());
                    let fresh = crate::scope::handed(door.types, door.bump, term, &mut |term| {
                        Some(spellings.named((door.types, door.bump), term).ty)
                    });
                    if fresh.map(DeclaredType::Type) != Some(image) {
                        broken.push(format!("{name:?} gives {image:?}, its term afresh {fresh:?}"));
                    }
                    if let DeclaredType::Type(image) = image
                        && let crate::type_lattice::TypeNode::Derived { term: id, .. } = door.types.node(image)
                        && let Some(held) = names(id)
                        && held.ty != image
                    {
                        broken.push(format!("{name:?} gives {image:?}, a table holds {held:?}"));
                    }
                }
                broken
            });
            prop_assert!(broken.is_empty(), "{:?} in\n{}", broken, case.render());
        }

        /// A derived name lies within its ends: each one the load or a resumption laid gives,
        /// under every binding that closes its term, a type between the closed ends of the node
        /// naming it.
        #[test]
        fn a_derived_name_lies_within_its_ends(case in resumable()) {
            let outside = resumed(&case.render(), |door| {
                let shapes = door.running.program().shapes();
                let levels = crate::scope::Levels::of(door.bump, shapes, binding(door, &case.bound));
                let resumed = resume_all(door, std::slice::from_ref(&case.bound), false).ok();
                let tables: Vec<_> = std::iter::once(door.load())
                    .chain(resumed)
                    .map(|facts| facts.base_layer().derived())
                    .collect();
                let names = |id| {
                    (resumed.and_then(|facts| facts.derived_name(id)))
                        .or_else(|| door.load().derived_name(id))
                };
                let binds = crate::scope::LevelsOver {
                    levels: &levels,
                    names: &names,
                    spellings: crate::scope::Spellings::new(door.symbols()),
                    laid: &|_| {},
                };
                let mut outside = Vec::new();
                for name in tables.iter().flat_map(|table| table.names()) {
                    let crate::scope::Closed::Type(DeclaredType::Type(applied)) =
                        crate::scope::rebound(door.types, door.bump, name.ty.into(), &binds)
                    else {
                        continue;
                    };
                    let crate::type_lattice::TypeNode::Derived { lower, bound, .. } = door.types.node(name.ty) else {
                        unreachable!("a derived name is a derived node")
                    };
                    let within = |a, b| crate::type_lattice::is_subtype_of(door.types, door.bump, a, b);
                    if !(within(lower, applied) && within(applied, bound)) {
                        outside.push(format!("{name:?} gives {applied:?}"));
                    }
                }
                outside
            });
            prop_assert!(outside.is_empty(), "{:?} in\n{}", outside, case.render());
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
                (door.load().own_units())
                    .flat_map(|(unit, (_, frontier))| {
                        frontier.unknowns().into_iter().filter(move |unknown| !on_chain(door, unit.segment, unknown.segment))
                            .map(move |unknown| format!("{unit:?} waits on {unknown:?}"))
                    })
                    .collect::<Vec<_>>()
            });
            prop_assert!(foreign.is_empty(), "{:?} in\n{}", foreign, case.render());
        }

        /// 4. A resumption is monotone: each fact it re-runs lies within the load's with the
        /// binding substituted, a verdict of *always* stays, a narrowing only shrinks, and a
        /// settled site stays settled. A fact typed `Never` never arrives: it is the bottom, as is
        /// every fact of a quote's code the binding refuses.
        #[test]
        fn a_resumption_is_monotone(case in resumable()) {
            let broken = resumed(&case.render(), |door| monotone(door, &case.bound));
            prop_assert!(broken.is_empty(), "{:?} in\n{}", broken, case.render());
        }

        /// 5. A resumption re-runs exactly the units whose frontier at the load reaches an open
        /// mark on a name its binding binds, each once: so it terminates. A unit of a quote's code
        /// the binding refuses that it marks unreached ran to no record: the refusal cut it off.
        #[test]
        fn a_resumption_reruns_only_the_units_it_wakes(case in resumable()) {
            let compared = resumed(&case.render(), |door| {
                let binding = binding(door, &case.bound);
                let resumed = (door.running).resume::<Koan>(door.types, None, binding, false, door.bump).ok()?;
                let reran: Vec<_> = resumed.own_units().map(|(unit, _)| unit).collect();
                let woken: Vec<_> = (door.load().own_units())
                    .filter(|(_, (_, frontier))| frontier.meets_open(binding))
                    .map(|(unit, _)| unit)
                    .collect();
                let woken = uncut(woken, resumed.unreached_units());
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

        /// 14. A unit record not open commutes with binding: each line of `SUBJECT`'s render that a
        /// unit holds whose frontier reaches no open mark on a name the binding binds, read with
        /// the binding's levels written in by the test's own substitution, is the restart's. A
        /// restart that refuses, refusing in a unit that re-runs, is refused by the resumption
        /// alike.
        #[test]
        fn a_unit_record_not_open_commutes_with_binding(case in resumable()) {
            let lines = resumed(&case.render(), |door| commuted(door, &case.bound));
            let restart = case.restart(&case.bound);
            match loaded_rendered(&restart, &case.bound) {
                Ok(restarted) => for line in lines {
                    prop_assert!(restarted.contains(&line), "{} not in the restart\n{}\nrestarted as\n{}", line, case.render(), restart);
                },
                Err(refusal) => {
                    let resumed = resume_rendered(&case, std::slice::from_ref(&case.bound), false);
                    prop_assert_eq!(resumed.err(), Some(refusal), "in\n{}\nrestarted as\n{}", case.render(), restart);
                }
            }
        }

        /// 8. Every store gives equal facts under one binding: a load's store binding it from the
        /// start, a layer over the load, a settlement over the load binding each module by the tree
        /// the load read for it, and a frame whose slots hold what it binds — read off the top
        /// level once the program has run, each module bound by its digest, as a frame binds one —
        /// render alike, refusals included.
        #[test]
        fn every_store_gives_equal_facts(case in resumable()) {
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
            let [(_, layered), (_, structural)] = stores;
            // A settlement binds a module by the tree the load read for it, as the structural layer
            // does.
            prop_assert_eq!(&settled_rendered(&case), &structural, "in\n{}", case.render());
            // A frame's activation binds a module by its digest, as the layer does, and reads its
            // records again from the frame's region.
            prop_assert_eq!(&activated_rendered(&case, true), &layered, "in\n{}", case.render());
            let mut substrate = CellSubstrate::load::<Koan>(&case.render(), "<test>", 8, BuildBudget::default(), output())
                .expect("the program loads");
            let framed = substrate.with(|running| {
                running.ran().expect("the program runs");
                let (root, symbols) = (running.program().shapes().root(), running.symbols());
                let body = subject(root);
                let slots: Vec<_> = (case.bound.entries().iter())
                    .map(|(name, _)| {
                        let at = slot(symbols, body, name).expect("the unknown is `SUBJECT`'s");
                        let unknown = Unknown { segment: body.segment(), at: Address::Slot(at) };
                        let named = match *name {
                            "m" | "n" => Resumable::stamped(name),
                            _ => Resumable::declared(name),
                        };
                        (unknown, named)
                    })
                    .collect();
                framed::<Koan>(running, slots, render_facts, left_out(&case.bound))
            });
            let framed = framed.map_err(|error| unlocated(&error));
            prop_assert_eq!(&layered, &framed, "in\n{}", case.render());
        }

        /// 12. A settlement holds the facts a load with its binding declared computes: settling
        /// `SUBJECT` under the binding renders every fact of its body as the restart does,
        /// refusals worded alike.
        #[test]
        fn a_settlement_holds_the_facts_a_load_with_its_binding_declared_computes(case in resumable()) {
            let restart = case.restart(&case.bound);
            prop_assert_eq!(settled_rendered(&case), loaded_rendered(&restart, &case.bound), "in\n{}\nrestarted as\n{}", case.render(), restart);
        }

        /// 13. An activation equals a restart with its bindings declared: deciding `SUBJECT`'s
        /// entry in its frame's activation renders every fact of its body as the restart does,
        /// refusals worded alike.
        #[test]
        fn an_activation_equals_a_restart_with_its_bindings_declared(case in resumable()) {
            let restart = case.restart(&case.bound);
            prop_assert_eq!(activated_rendered(&case, false), loaded_rendered(&restart, &case.bound), "in\n{}\nrestarted as\n{}", case.render(), restart);
        }

        /// 10. A rooted resumption is the program's on its bodies: resuming from `SUBJECT`'s body
        /// down renders every fact of it as resuming the whole program does, refusals included.
        #[test]
        fn a_rooted_resumption_equals_the_programs_resumption_on_its_bodies(case in resumable()) {
            let (rooted, whole) = resumed(&case.render(), |door| {
                let binding = binding(door, &case.bound);
                let at = subject(door.root()).segment();
                let render = |facts: Result<&LoadFacts<'_>, ShapeError<'_>>| match facts {
                    Ok(facts) => Ok(rendered(door, facts, &case.bound)),
                    Err(error) => Err(error.display(door.symbols(), door.types).to_string()),
                };
                (
                    render((door.running).resume_at::<Koan>(door.types, binding, at, door.bump)),
                    render((door.running).resume::<Koan>(door.types, None, binding, false, door.bump)),
                )
            });
            prop_assert_eq!(rooted, whole, "in\n{}", case.render());
        }

        /// 11. A rooted resumption visits no unit outside its subtree: each unit it runs or
        /// absorbs in its own order is one of an owning body at or below `SUBJECT`'s, each once,
        /// and it reads the chain around it whole.
        #[test]
        fn a_rooted_resumption_visits_no_unit_outside_its_subtree_and_chain_prefix(case in resumable()) {
            let outside = resumed(&case.render(), |door| {
                let binding = binding(door, &case.bound);
                let at = subject(door.root()).segment();
                VISITED.with(|visited| visited.borrow_mut().clear());
                let _ = crate::dispatch::walk::visiting(visited, || {
                    (door.running).resume_at::<Koan>(door.types, binding, at, door.bump)
                });
                let visited = VISITED.with(|visited| visited.take());
                let mut seen = std::collections::HashSet::new();
                let entry = crate::scope::UnitId { segment: at, unit: crate::scope::UnitIndex::ENTRY };
                let mut outside: Vec<_> = (visited.iter())
                    .filter(|unit| !on_chain(door, unit.segment, at) || !seen.insert(**unit))
                    .map(|unit| format!("{unit:?}"))
                    .collect();
                if !visited.contains(&entry) {
                    outside.push("not `SUBJECT`'s entry".to_string());
                }
                outside
            });
            prop_assert!(outside.is_empty(), "{:?} visited past the subtree in\n{}", outside, case.render());
        }

        /// 9. Levels and path identities depend on the shape alone: every path entry a record
        /// holds at a level the shape numbers is the path the shape spells there, at the load and
        /// under the binding, so no record changes a level. A derived name is named by its term's
        /// content: each one a walk laid carries the id its term digests to. The shape's own
        /// numbering is `scope`'s `two_builds_of_one_text_agree`.
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
                for (at, facts) in facts.iter().enumerate() {
                    for name in facts.base_layer().derived().names() {
                        let named = match door.types.node(name.ty) {
                            crate::type_lattice::TypeNode::Derived { term, .. } => Some(term),
                            _ => None,
                        };
                        if name.id != name.term.id() || named != Some(name.id) {
                            broken.push(format!("records {at} name {name:?} by another id"));
                        }
                    }
                }
                for shape in shapes.bodies() {
                    let root = shapes.shape_at(shapes.chain_root(shape.segment()));
                    for (index, spelled) in shape.spelled().iter().enumerate() {
                        let (Some(id), Some(level)) = (spelled.id, shapes.path_level(shape.segment(), index)) else {
                            continue;
                        };
                        for (at, facts) in facts.iter().enumerate() {
                            let name = crate::scope::PathName::Spelled(level);
                            if let Some(entry) = facts.path(door.types, root, name).map(|(entry, _)| entry)
                                && (entry.root != id.root || entry.chain != id.chain || entry.label != id.label || entry.name != name)
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
            let Ok(mut substrate) = CellSubstrate::load::<Koan>(&restart, "<test>", 8, BuildBudget::default(), output()) else {
                return Ok(());
            };
            let restarted = substrate.with(|running| {
                let bump = Bump::new();
                let types = reading(running.program().tiers(), &bump);
                let door = Door { running, types: &types, bump: &bump };
                classes(&door, door.load())
            });
            // A refused quote's code never runs, nor any body it nests: each is vacuously sealed.
            let refused: Vec<String> = (sealed.iter().chain(restarted.iter()))
                .filter(|(_, class)| class.is_none())
                .map(|(path, _)| format!("{path}/"))
                .collect();
            let run = |classes: Vec<(String, Option<bool>)>| -> Vec<(String, Option<bool>)> {
                (classes.into_iter())
                    .filter(|(path, _)| !refused.iter().any(|under| path.starts_with(under.as_str())))
                    .collect()
            };
            let (sealed, restarted) = (run(sealed), run(restarted));
            // The restart's own `LET`s birth no body, so both hold one frame body at each path.
            let paths = |classes: &[(String, Option<bool>)]| classes.iter().map(|(path, _)| path.clone()).collect::<Vec<_>>();
            prop_assert_eq!(paths(&sealed), paths(&restarted), "in\n{}\nrestarted as\n{}", case.render(), restart);
            for ((path, was), (_, is)) in sealed.iter().zip(restarted.iter()) {
                if *was == Some(true) {
                    prop_assert!(*is != Some(false), "{} is porous in the restart\n{}\nrestarted as\n{}", path, case.render(), restart);
                }
            }
        }
    }

    /// `woken` but the units of `cut`, which a walk marked unreached: a refusal before or in them
    /// left them no record.
    fn uncut(
        woken: Vec<crate::scope::UnitId>,
        cut: impl Iterator<Item = crate::scope::UnitId>,
    ) -> Vec<crate::scope::UnitId> {
        let cut: Vec<_> = cut.collect();
        woken
            .into_iter()
            .filter(|unit| !cut.contains(unit))
            .collect()
    }

    /// Whether each frame body of `SUBJECT`'s subtree is sealed, by path: each step a nested
    /// body's place in program order. A quote's code the load refuses is neither (`None`).
    fn classes(door: &Door<'_, '_>, facts: &LoadFacts<'_>) -> Vec<(String, Option<bool>)> {
        fn walk(
            facts: &LoadFacts<'_>,
            shape: &BodyShape<'_>,
            path: String,
            out: &mut Vec<(String, Option<bool>)>,
        ) {
            if facts
                .closing(shape.segment())
                .is_some_and(|(closing, _)| closing.refusal.is_some())
            {
                out.push((path, None));
                return;
            }
            if let Some(class) = facts.class(shape) {
                out.push((
                    path.clone(),
                    Some(matches!(class, BodyClass::Sealed { .. })),
                ));
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

    /// `value` with each level `levels` binds read as its type, each derived name its term handed
    /// again with them written in, where every name it reads is bound: `None` where one is not.
    pub(super) fn substitute(
        door: &Door<'_, '_>,
        levels: &[(usize, KType)],
        value: Parametric,
    ) -> Option<Parametric> {
        let scratch = Bump::new();
        let bound = crate::scope::Levels::from_bound(&scratch, levels);
        let names = |id| door.load().derived_name(id);
        let binds = crate::scope::LevelsOver {
            levels: &bound,
            names: &names,
            spellings: crate::scope::Spellings::new(door.symbols()),
            laid: &|_| {},
        };
        let read = match crate::scope::rebound(door.types, &scratch, value.into(), &binds) {
            crate::scope::Closed::Same => value,
            crate::scope::Closed::Type(DeclaredType::Type(closed)) => closed.into(),
            crate::scope::Closed::Partial(DeclaredType::Type(partial)) => partial,
            _ => unreachable!("a type substitutes to a type"),
        };
        // A path's variable is no level a binding binds.
        let open = (crate::scope::free_variables(door.types, &scratch, read.into()).iter()).any(
            |variable| {
                matches!(
                    variable,
                    Variable::Lexical { .. } | Variable::Path { .. } | Variable::Derived { .. }
                )
            },
        );
        (!open).then_some(read)
    }

    /// The lines of `SUBJECT`'s render of each type fact the load fixes rigid — a type expression's
    /// type, a nested callable's type, a type binder's declaration, a registration's bucket entry —
    /// read with the binding `bound` substituted, where it binds every name the fact reads: what
    /// the run solves at its use.
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
            expressions.sort_by_key(|expression| expression.site);
            for (index, expression) in expressions.into_iter().enumerate() {
                if let Static::Rigid { value, .. } =
                    facts.typed_expression(door.types, shape, expression.site)
                    && let Some(read) = read(DeclaredType::Type(value))
                {
                    out.push(format!("{path} type {index} = closed {read}"));
                }
            }
            if shape.kind() == ShapeKind::Callable
                && !path.is_empty()
                && let Static::Rigid { value, .. } = facts.callable_type(door.types, shape)
                && let Some(read) = read(value.ktype)
            {
                out.push(format!("{path} callable closed {read}"));
            }
            for at in (0..shape.slots()).map(|at| Slot(at as u32)) {
                if let BinderSymbol::Type(name) = shape.slot_name(at)
                    && let Static::Rigid { value, .. } = facts.declared_type(door.types, shape, at)
                    && let Some(read) = read(DeclaredType::Type(value))
                {
                    out.push(format!(
                        "{path} {} declares closed {read}",
                        symbols.display(name.symbol())
                    ));
                }
                if let BinderSymbol::Registration(name) = shape.slot_name(at)
                    && let Static::Rigid { value, .. } =
                        facts.registered_type(door.types, shape, at)
                    && let Some(read) = read(value.shape)
                {
                    out.push(format!(
                        "{path} {} registers closed {read}",
                        symbols.display(name.symbol())
                    ));
                }
            }
        }
        out.iter().map(|line| canonical(line)).collect()
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
                        .statement_type(door.types, shape, index)
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
            // A quote's code the binding refuses never runs, nor any body it nests: the bottom.
            if (resumed.closing(shape.segment()))
                .is_some_and(|(closing, _)| closing.refusal.is_some())
            {
                continue;
            }
            shapes.extend(shape.nested_shapes().iter().map(|(_, nested)| *nested));
            for at in (0..shape.slots()).map(|at| Slot(at as u32)) {
                if let (Some(DeclaredType::Type(was)), Some(DeclaredType::Type(is))) = (
                    load.binder_type(door.types, shape, at),
                    resumed.binder_type(door.types, shape, at),
                ) {
                    broken.extend(outside(format!("binder {}", at.index()), was, is));
                }
            }
            for index in 0..shape.body().len() {
                if let (Some(was), Some(is)) = (
                    load.statement_type(door.types, shape, index),
                    resumed.statement_type(door.types, shape, index),
                ) {
                    broken.extend(outside(format!("statement {index}"), was, is));
                }
            }
            // A use or a check in a statement that never completes never runs.
            let vacuous = |site: Site| {
                (shape.body().iter().enumerate()).any(|(index, statement)| {
                    resumed
                        .statement_type(door.types, shape, index)
                        .is_some_and(never)
                        && source_of(shape, statement, site).is_some()
                })
            };
            for statement in shape.body() {
                let mut sites = vec![shape.site_of_node(statement)];
                each_part(statement, |part| {
                    sites.push(shape.site(part));
                    if let ExpressionPart::Expression(node) = part {
                        sites.push(shape.site_of_node(node));
                    }
                });
                for site in sites {
                    if let (Some(was), Some(is)) = (
                        load.value_type(door.types, shape, site),
                        resumed.value_type(door.types, shape, site),
                    ) {
                        broken.extend(outside(format!("part {site:?}"), was, is));
                    }
                    if vacuous(site) {
                        continue;
                    }
                    if shape.candidate_lists().get(site).is_some()
                        && let Some(why) = widened(
                            load.use_statics(door.types, &crate::memory::Bump::new(), shape, site)
                                .narrowing,
                            resumed
                                .use_statics(door.types, &crate::memory::Bump::new(), shape, site)
                                .narrowing,
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
                    kept.entries
                        .iter()
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

    /// What the witnessed laws read of a [`Witnessable`]: the layer its load witnessed under the
    /// case's binding `a`, and `b`, the binding they compare under — each type name an alternate
    /// keeping every bare question on it, else `a`'s, `Pair` following `Elt` and each module `a`'s
    /// — beside its description, whether `b` keeps every question, and whether it differs from `a`
    /// on a name some question reads.
    struct Witnessed<'d> {
        witnessed: &'d crate::scope::BuildLayer<'d>,
        first: crate::scope::BuildLayer<'d>,
        a: crate::scope::Binding<'d>,
        b: crate::scope::Binding<'d>,
        b_bound: Bound,
        kept: bool,
        differs: bool,
    }

    /// The level the type name `name` of `SUBJECT` is homed at, where it has one.
    fn level_of(door: &Door<'_, '_>, name: &str) -> Option<usize> {
        let body = subject(door.root());
        let at = slot(door.symbols(), body, name)?;
        (door.load().homes(body).iter())
            .find(|(_, target)| *target == Target::Local(at))
            .map(|(level, _)| *level)
    }

    /// Whether `answered` is a bare question on the name homed at `level`: it reads no other
    /// name, and reads that one as a whole type.
    fn bare_on(door: &Door<'_, '_>, answered: &crate::scope::Answered<'_>, level: usize) -> bool {
        let scratch = Bump::new();
        let (mut whole, mut alone) = (false, true);
        answered.question.each_type(&mut |ty| {
            if let DeclaredType::Type(typed) = ty
                && matches!(door.types.node(typed),
                    crate::type_lattice::TypeNode::Lexical { level: held, .. } if held == level)
            {
                whole = true;
            }
            for variable in crate::scope::free_variables(door.types, &scratch, ty) {
                alone &= matches!(variable, Variable::Lexical { level: held, .. } if held == level);
            }
        });
        whole && alone
    }

    /// The closed type `binding` gives each type name's level of `SUBJECT`'s chain.
    fn levels_of(door: &Door<'_, '_>, binding: crate::scope::Binding<'_>) -> Vec<(usize, KType)> {
        let body = subject(door.root());
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

    /// Whether a binding whose levels are `levels` keeps `answered`, a question of `layer`: each
    /// derived name it holds read off the tables of `layer`, those beneath and the load's, a term
    /// they lack named afresh.
    fn keeps_under(
        door: &Door<'_, '_>,
        layer: &crate::scope::BuildLayer<'_>,
        answered: &crate::scope::Answered<'_>,
        levels: &[(usize, KType)],
    ) -> bool {
        let scratch = Bump::new();
        let levels = crate::scope::Levels::from_bound(&scratch, levels);
        let names = |id| (layer.derived_name(id)).or_else(|| door.load().derived_name(id));
        let binds = crate::scope::LevelsOver {
            levels: &levels,
            names: &names,
            spellings: crate::scope::Spellings::new(door.symbols()),
            laid: &|_| {},
        };
        crate::scope::keeps(door.types, &scratch, answered, &binds)
    }

    /// `case`'s load witnessed under its binding, and the binding the witnessed laws compare under:
    /// `None` where the witnessing walk refused or witnessed nothing.
    fn witnessed<'d>(door: &Door<'d, '_>, case: &Witnessable) -> Option<Witnessed<'d>> {
        let a = binding(door, &case.case.bound);
        let first = door.resumption(a, None, false).ok()?;
        let witnessed = first.witnessed?;
        let mut questions = Vec::new();
        witnessed
            .admission()
            .each_kept(&mut |answered| questions.push(answered));
        let body = subject(door.root());
        let (mut chosen, mut rebound, mut differs) = (Vec::new(), Vec::new(), false);
        for name in ALTERNATED {
            let (Some(level), Some((_, _))) = (
                level_of(door, name),
                case.case
                    .bound
                    .entries()
                    .into_iter()
                    .find(|(bound, _)| *bound == name),
            ) else {
                continue;
            };
            let unknown = Unknown {
                segment: body.segment(),
                at: Address::Slot(slot(door.symbols(), body, name).expect("a type name's slot")),
            };
            let Some(Bind::Type(was)) = a.bound(unknown) else {
                continue;
            };
            let bare: Vec<_> = (questions.iter())
                .filter(|answered| bare_on(door, answered, level))
                .collect();
            let read = (questions.iter()).any(|answered| {
                let scratch = Bump::new();
                let mut reads = false;
                answered.question.each_type(&mut |ty| {
                    reads |= (crate::scope::free_variables(door.types, &scratch, ty).iter())
                        .any(|variable| matches!(variable, Variable::Lexical { level: held, .. } if *held == level));
                });
                reads
            });
            let found = (0..3).find(|index| {
                let alternate = door.declared(&Witnessable::alternate(name, *index));
                (bare.iter())
                    .all(|answered| keeps_under(door, witnessed, answered, &[(level, alternate)]))
            });
            if let Some(index) = found {
                let alternate = door.declared(&Witnessable::alternate(name, index));
                differs |= read && alternate != was;
                chosen.push((name, index));
                rebound.push((unknown, alternate));
            }
        }
        let mut staged: Vec<_> = (a.entries().iter())
            .map(
                |(unknown, bound)| match rebound.iter().find(|(held, _)| held == unknown) {
                    Some((_, typed)) => (*unknown, Bind::Type(*typed)),
                    None => (*unknown, *bound),
                },
            )
            .collect();
        staged.sort_by_key(|(unknown, _)| *unknown);
        let b = door.bound(&staged);
        let levels = levels_of(door, b);
        let kept =
            (questions.iter()).all(|answered| keeps_under(door, witnessed, answered, &levels));
        Some(Witnessed {
            witnessed,
            first: first.layer,
            a,
            b,
            b_bound: case.bound(&chosen),
            kept,
            differs,
        })
    }

    /// The witnessed names of `witnessed`, as `binding` binds them: what a closing walk over it
    /// checks.
    fn checked_of<'d>(
        door: &Door<'d, '_>,
        witnessed: &crate::scope::BuildLayer<'d>,
        binding: crate::scope::Binding<'d>,
    ) -> crate::scope::Binding<'d> {
        let names = witnessed.witnessing().expect("a witnessed layer").names;
        let staged: Vec<_> = (names.iter())
            .filter_map(|unknown| Some((*unknown, binding.bound(*unknown)?)))
            .collect();
        door.bound(&staged)
    }

    /// The frontier of `unit` as a walk over `top`, a chain of layers laid over the load, reads
    /// it: the innermost layer's that ran it, else the load's.
    fn frontier_over<'d>(
        door: &Door<'d, '_>,
        top: &crate::scope::BuildLayer<'d>,
        unit: crate::scope::UnitId,
    ) -> Option<crate::scope::Frontier<'d>> {
        let record = (std::iter::successors(Some(top), |layer| layer.parent()))
            .find_map(|layer| layer.unit(unit));
        match record {
            Some(record) => record.typed(),
            None => door.load().unit(unit),
        }
        .map(|(_, frontier)| frontier)
    }

    /// The units a closing walk under `binding` over `witnessed` re-runs, by the flattened oracle:
    /// each whose frontier over it reaches a question on a checked name the binding does not keep,
    /// and each the layer marks unreached whose running unit re-runs, which may reach it now.
    fn woken_over<'d>(
        door: &Door<'d, '_>,
        witnessed: &crate::scope::BuildLayer<'d>,
        binding: crate::scope::Binding<'d>,
    ) -> Vec<crate::scope::UnitId> {
        woken_through(
            door,
            witnessed,
            witnessed,
            binding,
            &levels_of(door, binding),
        )
    }

    /// [`woken_over`] over `top`, a chain holding `witnessed`, the binding's levels `levels`.
    fn woken_through<'d>(
        door: &Door<'d, '_>,
        witnessed: &crate::scope::BuildLayer<'d>,
        top: &crate::scope::BuildLayer<'d>,
        binding: crate::scope::Binding<'d>,
        levels: &[(usize, KType)],
    ) -> Vec<crate::scope::UnitId> {
        let checked = checked_of(door, witnessed, binding);
        let keeps =
            |answered: &crate::scope::Answered<'_>| keeps_under(door, top, answered, levels);
        let units: Vec<_> = door.load().own_units().map(|(unit, _)| unit).collect();
        let mut woken: Vec<_> = (units.iter().copied())
            .filter(|unit| {
                frontier_over(door, top, *unit).is_some_and(|frontier| {
                    frontier.wakes_flat(checked, crate::scope::Binding::EMPTY, binding, &keeps)
                        == crate::scope::Wake::Open
                })
            })
            .collect();
        // A unit a layer marks unreached runs again wherever the unit running it does.
        let shapes = door.running.program().shapes();
        let running = |unit: crate::scope::UnitId| {
            let (parent, statement) = shapes.parent(unit.segment)?;
            Some(crate::scope::UnitId {
                segment: parent,
                unit: shapes
                    .shape_at(parent)
                    .unit_of_statement(statement as usize),
            })
        };
        loop {
            let reached: Vec<_> = (units.iter().copied())
                .filter(|unit| !woken.contains(unit) && frontier_over(door, top, *unit).is_none())
                .filter(|unit| running(*unit).is_some_and(|at| woken.contains(&at)))
                .collect();
            if reached.is_empty() {
                break;
            }
            woken.extend(reached);
        }
        woken.sort();
        woken
    }

    /// The lines of `SUBJECT`'s render over `witnessed` that units a closing walk under `b` does
    /// not re-run hold, each type read with `b`'s levels written in: law 14's side, widened.
    fn commuted_over<'d>(door: &Door<'d, '_>, witnessed: &Witnessed<'d>) -> Vec<String> {
        let facts = crate::scope::Facts::under(door.load(), Some(witnessed.witnessed));
        let levels = levels_of(door, witnessed.b);
        let open = woken_over(door, witnessed.witnessed, witnessed.b);
        commuted_in(door, &facts, levels, open, &witnessed.b_bound)
    }

    /// The lines of `SUBJECT`'s render of `facts` that units not in `open` hold, each type read
    /// with `levels` written in, but of the binders `bound` names.
    fn commuted_in<'f, S: RecordSource<'f>>(
        door: &Door<'_, '_>,
        facts: &FactsOver<S>,
        levels: Vec<(usize, KType)>,
        open: Vec<crate::scope::UnitId>,
        bound: &Bound,
    ) -> Vec<String> {
        let load = door.load();
        let mut substituting = Substituting {
            levels,
            sites: std::collections::HashMap::new(),
            born: std::collections::HashMap::new(),
            open,
        };
        for (unit, (facts, _)) in load.own_units() {
            // A tree written over names closes by tree substitution, which a walk does.
            if super::writes_tree_names(door.types, facts) {
                substituting.open.push(unit);
            }
            for (segment, typed) in facts.typed.iter() {
                typed.held(*segment, &mut |held| {
                    if let crate::scope::Held::Site(segment, site) = held {
                        substituting.sites.insert((segment, site), unit);
                    }
                });
            }
            for (segment, _) in facts.born.iter() {
                substituting.born.insert(*segment, unit);
            }
        }
        let names = left_out(bound);
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let reader = Reader {
            facts,
            types: door.types,
            symbols: door.symbols(),
            bound: &names,
            substituting: Some(&substituting),
            closes: std::cell::Cell::new(true),
        };
        let mut out = Vec::new();
        reader.render(subject(load.shapes().root()), "", &mut out);
        out.iter().map(|line| canonical(line)).collect()
    }

    /// The closing walk under `b` over the witnessed layer: its render, or its refusal.
    fn over_rendered<'d>(
        door: &Door<'d, '_>,
        witnessed: &Witnessed<'d>,
    ) -> Result<Vec<String>, String> {
        let resumed = door.resumption(witnessed.b, Some(witnessed.witnessed), false);
        let layer = resumed.map_err(|refusal| unlocated(&refusal))?.layer;
        let facts = crate::scope::Facts::under(
            door.load(),
            Some(crate::memory::resident(door.bump, layer)),
        );
        Ok(render_over(
            &facts,
            door.types,
            door.symbols(),
            &left_out(&witnessed.b_bound),
        ))
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: crate::tests::case_share(1, 4),
            ..ProptestConfig::default()
        })]

        /// 14, widened. A unit record whose frontier over a witnessed layer reaches no question
        /// a binding does not keep commutes with that binding: each line of `SUBJECT`'s render
        /// over the layer such a unit holds, read with the binding's levels written in, is the
        /// restart's. A restart that refuses is refused alike by the closing walk over the layer.
        #[test]
        fn a_unit_record_whose_questions_a_binding_keeps_commutes_with_it(case in witnessable()) {
            let read = resumed(&case.render(), |door| {
                let witnessed = witnessed(door, &case)?;
                Some((
                    witnessed.kept,
                    witnessed.b_bound.clone(),
                    commuted_over(door, &witnessed),
                    over_rendered(door, &witnessed).err(),
                ))
            });
            let Some((kept, b, lines, refused)) = read else {
                return Ok(());
            };
            prop_assume!(kept);
            let restart = case.case.restart(&b);
            match loaded_rendered(&restart, &b) {
                Ok(restarted) => for line in lines {
                    prop_assert!(restarted.contains(&line), "{} not in the restart\n{}\nrestarted as\n{}", line, case.render(), restart);
                },
                Err(refusal) => prop_assert_eq!(refused, Some(refusal), "in\n{}\nrestarted as\n{}", case.render(), restart),
            }
        }

        /// 8, witnessed. A binding read over the layer another binding witnessed renders as the
        /// restart under it, refusals included.
        #[test]
        fn a_resumption_over_a_witnessed_layer_is_a_restart(case in witnessable()) {
            let read = resumed(&case.render(), |door| {
                let witnessed = witnessed(door, &case)?;
                Some((witnessed.b_bound.clone(), over_rendered(door, &witnessed)))
            });
            let Some((b, over)) = read else {
                return Ok(());
            };
            let restart = case.case.restart(&b);
            prop_assert_eq!(over, loaded_rendered(&restart, &b), "in\n{}\nrestarted as\n{}", case.render(), restart);
        }

        /// 5, witnessed. A witnessing walk re-runs exactly the units whose frontier at the load
        /// reaches an undecided question on a witnessed name and no undecided one of a holding
        /// kind, or one on a module binder it binds; a closing walk over its layer, exactly those
        /// whose frontier over the layer reaches a question on a checked name its binding does not
        /// keep, or which the layer marks unreached and a re-run unit runs — under the witness,
        /// and under another binding. Each walk's units it marks unreached ran to no record, as in
        /// law 5.
        #[test]
        fn a_witnessed_resumption_reruns_only_the_units_it_wakes(case in witnessable()) {
            let compared = resumed(&case.render(), |door| {
                let witnessed = witnessed(door, &case)?;
                let mut names: Vec<_> = (witnessed.a.entries().iter())
                    .filter(|(_, bound)| matches!(bound, Bind::Type(_)))
                    .copied()
                    .collect();
                names.sort_by_key(|(unknown, _)| *unknown);
                let witness = door.bound(&names);
                let mut modules: Vec<_> = (witnessed.a.entries().iter())
                    .filter(|(_, bound)| matches!(bound, Bind::Module { .. }))
                    .copied()
                    .collect();
                modules.sort_by_key(|(unknown, _)| *unknown);
                let modules = door.bound(&modules);
                let first: Vec<_> = witnessed.witnessed.own_units().collect();
                let wakes = |unit: crate::scope::UnitId| {
                    (door.load().own_units()).any(|(at, (_, frontier))| {
                        at == unit
                            && frontier.wakes_flat(modules, witness, modules, &|_| false)
                                == crate::scope::Wake::Open
                    })
                };
                // A unit of an inline body is absorbed with the unit running it, whose record reads
                // its own.
                let shapes = door.running.program().shapes();
                let running = |unit: crate::scope::UnitId| {
                    let shape = shapes.shape_at(unit.segment);
                    if !matches!(shape.kind(), ShapeKind::Block | ShapeKind::Module) {
                        return None;
                    }
                    let (parent, statement) = shapes.parent(unit.segment)?;
                    Some(crate::scope::UnitId {
                        segment: parent,
                        unit: shapes.shape_at(parent).unit_of_statement(statement as usize),
                    })
                };
                let woken_first: Vec<_> = (door.load().own_units())
                    .map(|(unit, _)| unit)
                    .filter(|unit| {
                        std::iter::successors(Some(*unit), |at| running(*at)).all(wakes)
                    })
                    .collect();
                let woken_first = uncut(woken_first, witnessed.witnessed.unreached_units());
                let closing: Vec<_> = witnessed.first.own_units().collect();
                let woken_closing = uncut(
                    woken_over(door, witnessed.witnessed, witnessed.a),
                    witnessed.first.unreached_units(),
                );
                let over = door.resumption(witnessed.b, Some(witnessed.witnessed), false).ok();
                let over: Option<(Vec<_>, Vec<_>)> = over.map(|resumed| {
                    (resumed.layer.own_units().collect(), resumed.layer.unreached_units().collect())
                });
                let woken_over = woken_over(door, witnessed.witnessed, witnessed.b);
                let (over, woken_over) = match over {
                    Some((over, cut)) => (Some(over), uncut(woken_over, cut.into_iter())),
                    None => (None, woken_over),
                };
                Some(((first, woken_first), (closing, woken_closing), (over, woken_over)))
            });
            if let Some(((first, woken_first), (closing, woken_closing), (over, woken))) = compared {
                prop_assert_eq!(first, woken_first, "the witnessing walk in\n{}", case.render());
                prop_assert_eq!(closing, woken_closing, "the closing walk in\n{}", case.render());
                // A closing walk that refuses re-runs nothing to compare: law 8 holds its refusal.
                if let Some(over) = over {
                    prop_assert_eq!(over, woken, "the closing walk under another binding in\n{}", case.render());
                }
            }
        }
    }

    /// The witnessed laws compare some binding that differs from the witness on a name a question
    /// reads: on a fixed seed, one of 64 cases does.
    #[test]
    fn the_witnessed_laws_draw_a_binding_other_than_the_witness() {
        use proptest::strategy::ValueTree;
        use proptest::test_runner::{RngAlgorithm, TestRng, TestRunner};
        let mut runner = TestRunner::new_with_rng(
            ProptestConfig::default(),
            TestRng::deterministic_rng(RngAlgorithm::ChaCha),
        );
        let strategy = witnessable();
        let differs = (0..64).any(|_| {
            let case = strategy.new_tree(&mut runner).expect("a case").current();
            resumed(&case.render(), |door| {
                witnessed(door, &case).is_some_and(|witnessed| witnessed.differs && witnessed.kept)
            })
        });
        assert!(differs, "no case binds an alternate a question reads");
    }

    /// The resumption laws draw a `:Module` parameter, and one declared at a meet, that a binding
    /// binds and a statement reads: on a fixed seed, one of 128 cases does, for each.
    #[test]
    fn the_resumption_laws_draw_a_bound_module_parameter() {
        use super::super::generate::ModuleAt;
        use proptest::strategy::ValueTree;
        use proptest::test_runner::{RngAlgorithm, TestRng, TestRunner};
        for at in [ModuleAt::Module, ModuleAt::Meet] {
            let mut runner = TestRunner::new_with_rng(
                ProptestConfig::default(),
                TestRng::deterministic_rng(RngAlgorithm::ChaCha),
            );
            let strategy = super::super::generate::resumable();
            let drawn = (0..128).any(|_| {
                let case = strategy.new_tree(&mut runner).expect("a case").current();
                let rendered = case.render();
                case.module_at == at
                    && case.bound.module.is_some()
                    && ["m.zero", "(m :|", "(m :!"]
                        .iter()
                        .any(|read| rendered.contains(read))
            });
            assert!(
                drawn,
                "no case binds an `m` declared at {at:?} that it reads"
            );
        }
    }

    /// The `CELL` registration `SUBJECT`'s body holds, where a [`Statement::Cell`] wrote one.
    fn cell<'graph>(door: &Door<'_, 'graph>) -> Option<(usize, &'graph BodyShape<'graph>)> {
        (subject(door.root()).nested_shapes().iter().enumerate())
            .map(|(index, (_, nested))| (index, *nested))
            .find(|(_, nested)| {
                nested.kind() == ShapeKind::Callable
                    && slot(door.symbols(), nested, "Cel").is_some()
            })
    }

    /// What the commutation law over an encloser and a cell reads of a [`cellular`] case: the
    /// binding `b` it compares under, which keeps every question of both witnessed layers;
    /// whether `b` binds a name some question reads to another type than the witness; the lines
    /// of `CELL`'s render over its witnessed layer that commute; and the closing walk's refusal
    /// under `b`. `None` where an event refused or witnessed nothing.
    struct CellRead {
        b: Bound,
        differs: bool,
        lines: Vec<String>,
        refused: Option<String>,
    }

    /// [`CellRead`]: `SUBJECT`'s entry activated under the case's binding, witnessing; `CELL`'s
    /// entry over its binding layer binding `Cel`, witnessing; `b` the first of the alternates for
    /// `Elt` (where the case binds it) and `Cel` under which both witnessed layers keep every
    /// question, one that differs where any does; `SUBJECT`'s entry under `b` over its witnessed
    /// layer; and `CELL`'s render over its witnessed layer laid over that.
    fn cell_read(door: &Door<'_, '_>, case: &Witnessable) -> Option<CellRead> {
        use crate::program::activation_store::Activating;
        use crate::scope::{EventSite, Keeping};
        let body = subject(door.root());
        let (index, cell) = cell(door)?;
        let (running, types) = (door.running, door.types);
        let names = ["Elt", "Key", "Item", "m", "n"];
        let a = bind(door, &case.case.bound.only(&names), true);
        let entry = |segment| Activating::Witnessing {
            site: EventSite::entry(segment),
        };
        let at = (body.segment(), cell.segment());
        let first =
            (running.activate_over::<Koan>(types, None, a, entry(at.0), at.0, door.bump)).ok()?;
        let encloser = first.witnessed?;
        let binding = crate::memory::resident(door.bump, first.layer);
        let cel_slot = slot(door.symbols(), cell, "Cel")?;
        let cel = Unknown {
            segment: cell.segment(),
            at: Address::Slot(cel_slot),
        };
        let witness = door.declared(&Resumable::declared("Cel"));
        let own = door.bound(&[(cel, Bind::Type(witness))]);
        let below = Some(binding);
        let witnessed =
            (running.activate_over::<Koan>(types, below, own, entry(at.1), at.1, door.bump))
                .ok()?
                .witnessed?;
        let cel_level = (door.load().homes(cell).iter())
            .find(|(_, target)| *target == Target::Local(cel_slot))
            .map(|(level, _)| *level)?;
        // Each witnessed layer's questions: the encloser's, judged under `SUBJECT`'s levels, and
        // the cell's, under `Cel`'s too.
        let (mut theirs, mut ours) = (Vec::new(), Vec::new());
        (encloser.admission()).each_kept(&mut |answered| theirs.push(answered));
        (witnessed.admission()).each_kept(&mut |answered| ours.push(answered));
        let questions: Vec<_> = theirs.iter().chain(&ours).copied().collect();
        let reads = |level: usize| {
            (questions.iter()).any(|answered| {
                let scratch = Bump::new();
                let mut reads = false;
                answered.question.each_type(&mut |ty| {
                    reads |= (crate::scope::free_variables(door.types, &scratch, ty).iter())
                        .any(|variable| matches!(variable, Variable::Lexical { level: held, .. } if *held == level));
                });
                reads
            })
        };
        let unknown = |name| {
            Some(Unknown {
                segment: body.segment(),
                at: Address::Slot(slot(door.symbols(), body, name)?),
            })
        };
        // Only a name the case's binding binds is alternated, as the witnessed laws draw.
        let elt =
            (level_of(door, "Elt").zip(unknown("Elt"))).filter(|_| case.case.bound.elt.is_some());
        // `SUBJECT`'s binding and `Cel`'s type under the alternates `chosen`, whether they bind a
        // name a question reads to another type than the witness, and the levels each binds.
        let under = |chosen: &[(&str, usize)]| {
            let mut staged: Vec<_> = a.entries().to_vec();
            let (mut cel_bound, mut differs) = (witness, false);
            for (name, found) in chosen {
                let alternate = door.declared(&Witnessable::alternate(name, *found));
                match (*name, elt) {
                    ("Elt", Some((level, elt))) => {
                        let was = (staged.iter()).find(|(unknown, _)| *unknown == elt);
                        differs |= reads(level)
                            && !matches!(was, Some((_, Bind::Type(was))) if *was == alternate);
                        staged.retain(|(unknown, _)| *unknown != elt);
                        staged.push((elt, Bind::Type(alternate)));
                    }
                    _ => {
                        differs |= reads(cel_level) && alternate != witness;
                        cel_bound = alternate;
                    }
                }
            }
            staged.sort_by_key(|(unknown, _)| *unknown);
            let subject_levels = levels_of(door, door.bound(&staged));
            let mut levels = subject_levels.clone();
            levels.push((cel_level, cel_bound));
            (staged, cel_bound, differs, (subject_levels, levels))
        };
        let keeps_all = |(subject, cell): &(Vec<_>, Vec<_>)| {
            (theirs.iter()).all(|answered| keeps_under(door, encloser, answered, subject))
                && (ours.iter()).all(|answered| keeps_under(door, witnessed, answered, cell))
        };
        let elts: &[Option<usize>] = match elt {
            Some(_) => &[None, Some(0), Some(1), Some(2)],
            None => &[None],
        };
        let mut drawn: Vec<Vec<(&str, usize)>> = Vec::new();
        for elt in elts {
            for cel in [None, Some(0), Some(1), Some(2)] {
                let chosen = [
                    elt.map(|found| ("Elt", found)),
                    cel.map(|found| ("Cel", found)),
                ];
                drawn.push(chosen.into_iter().flatten().collect());
            }
        }
        // The first combination both layers keep that differs, else the witness's own.
        let chosen = (drawn.into_iter())
            .find(|chosen| {
                let (_, _, differs, levels) = under(chosen);
                differs && keeps_all(&levels)
            })
            .unwrap_or_default();
        let (staged, cel_bound, differs, (_, levels)) = under(&chosen);
        let b_subject = door.bound(&staged);
        let b_cel = door.bound(&[(cel, Bind::Type(cel_bound))]);
        let mut every = staged.clone();
        every.push((cel, Bind::Type(cel_bound)));
        every.sort_by_key(|(unknown, _)| *unknown);
        let b_every = door.bound(&every);
        let b = case.bound(&chosen);
        let display = |refusal: &crate::scope::ShapeError<'_>| {
            unlocated(&refusal.display(door.symbols(), door.types).to_string())
        };
        let over = Activating::Over(encloser, Keeping::All);
        let resumed =
            match running.activate_over::<Koan>(types, None, b_subject, over, at.0, door.bump) {
                Ok(resumed) => crate::memory::resident(door.bump, resumed.layer),
                Err(refusal) => {
                    let refused = Some(display(refusal));
                    return Some(CellRead {
                        b,
                        differs,
                        lines: Vec::new(),
                        refused,
                    });
                }
            };
        let closing = Activating::Over(witnessed, Keeping::All);
        let refused =
            (running.activate_over::<Koan>(types, Some(resumed), b_cel, closing, at.1, door.bump))
                .err()
                .map(display);
        let top = crate::memory::resident(door.bump, witnessed.over(Some(resumed)));
        let open = woken_through(door, witnessed, top, b_every, &levels);
        let facts = crate::scope::Facts::of(door.load()).activated(Some(top));
        // `CELL`'s own lines; its callable type is `SUBJECT`'s fact, which a restart binding
        // `Cel` writes without its group.
        let path = format!("/{index}");
        let lines: Vec<String> = (commuted_in(door, &facts, levels, open, &b).into_iter())
            .filter(|line| {
                line.starts_with(&format!("{path} ")) || line.starts_with(&format!("{path}/"))
            })
            .filter(|line| !line.starts_with(&format!("{path} callable ")))
            .collect();
        Some(CellRead {
            b,
            differs,
            lines,
            refused,
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: crate::tests::case_share(1, 4),
            ..ProptestConfig::default()
        })]

        /// 14, over an encloser and a cell. A cell's unit record whose frontier over the cell's
        /// witnessed layer, laid over its encloser's binding layer under another binding, reaches
        /// no question that binding does not keep commutes with it: each line of `CELL`'s render
        /// such a unit holds, read with the binding's levels written in, is the restart's. A
        /// restart that refuses is refused alike by the closing walk.
        #[test]
        fn a_cells_unit_record_whose_questions_a_binding_keeps_commutes_with_it(case in cellular()) {
            let Some(read) = resumed(&case.render(), |door| cell_read(door, &case)) else {
                return Ok(());
            };
            let restart = case.case.restart(&read.b);
            match loaded_rendered(&restart, &read.b) {
                Ok(restarted) => for line in &read.lines {
                    prop_assert!(restarted.contains(line), "{} not in the restart\n{}\nrestarted as\n{}", line, case.render(), restart);
                },
                Err(refusal) => prop_assert_eq!(read.refused, Some(refusal), "in\n{}\nrestarted as\n{}", case.render(), restart),
            }
        }
    }

    /// The cell law compares some binding that differs from the witness on a name a question
    /// reads, and renders some line: on a fixed seed, one of 64 cases does.
    #[test]
    fn the_cell_law_draws_a_binding_other_than_the_witness() {
        use proptest::strategy::ValueTree;
        use proptest::test_runner::{RngAlgorithm, TestRng, TestRunner};
        let mut runner = TestRunner::new_with_rng(
            ProptestConfig::default(),
            TestRng::deterministic_rng(RngAlgorithm::ChaCha),
        );
        let strategy = cellular();
        let differs = (0..64).any(|_| {
            let case = strategy.new_tree(&mut runner).expect("a case").current();
            resumed(&case.render(), |door| {
                cell_read(door, &case).is_some_and(|read| read.differs && !read.lines.is_empty())
            })
        });
        assert!(differs, "no case binds an alternate a question reads");
    }
}

/// A field read of a variable whose bound guarantees the field names the variable and judges
/// nothing over it, so a binding absorbs the unit; over an unbounded variable `ATTR` may read a
/// module member, which the binding's module declares or not: a question, which holds nothing.
#[test]
fn a_field_read_through_a_guaranteeing_bound_names_its_variable() {
    let marks = |group: &str| {
        let source =
            format!("EXPR FOR ALL {group} #(GET x :Elt) -> Any = #(\n  LET y = x.f\n  y\n)");
        resumed(&source, |door| {
            let f = quantified(door);
            let elt = unknown(door.symbols(), f, "Elt");
            let y = unit_of(door, f, "y");
            waits_on(door, y, elt).unwrap_or_default()
        })
    };
    let bounded = marks("#{Elt: :{f :Number}}");
    assert!(
        bounded.iter().flatten().all(|kind| !kind.holds()),
        "{bounded:?}"
    );
    let unbounded = marks("#[Elt]");
    assert!(
        unbounded.iter().flatten().all(|kind| !kind.holds()),
        "{unbounded:?}"
    );
    assert!(
        unbounded.contains(&Some(crate::scope::OpenKind::Spells)),
        "{unbounded:?}"
    );
}

/// A field read of a variable is exact under a binding whose retype makes it so: bound to
/// `{f :Number}`, `x` is exactly that record, so `x.f` is exactly `Number`, where the load gave
/// `[Never, (FIELD f OF Elt)]`.
#[test]
fn a_field_read_of_a_variable_is_exact_under_a_binding() {
    let source = "EXPR FOR ALL #{Elt: :{f :Number}} #(GET x :Elt) -> Any = #(\n  \
                  LET y = x.f\n  y\n)";
    let (lower, upper) = resumed(source, |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let field = BinderSymbol::declared("f", door.symbols()).expect("a value token");
        let record = door.types.record(door.bump, &[(field, KType::NUMBER)]);
        let binding = door.binding(&[(elt, record)]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        let at = slot(door.symbols(), f, "y");
        let Some(DeclaredType::Type(typed)) = resumed.binder_type(door.types, f, at) else {
            panic!("`y` is typed, and no scheme");
        };
        let show = |end| display_name(end, door.types, door.symbols()).to_string();
        (show(typed.lower), show(typed.upper))
    });
    assert_eq!((lower.as_str(), upper.as_str()), ("Number", "Number"));
}

/// `SUBJECT`'s body: `m` constructs `Meters` over `y :Item`, and `t` reads `m` alone.
const METERS: &str = "#(SUBJECT y :Item) -> Any = #(\n  LET m = (Meters y)\n  LET t = m\n  t\n)";

/// A construction of a closed newtype over a run-bound payload is `Meters` at most where the
/// payload may not fit, and exactly `Meters` under `Item := Number`, as a fresh load with the
/// binding declared types it.
#[test]
fn a_construction_over_a_run_bound_payload_resumed_is_what_a_fresh_load_types() {
    let declared = ":Number";
    let source =
        format!("LET Bound = {declared}\nNEWTYPE Meters = Number\nEXPR FOR ALL #[Item] {METERS}");
    let under = resumed(&source, |door| {
        let f = subject(door.root());
        let item = unknown(door.symbols(), f, "Item");
        let binding = door.binding(&[(item, door.declared("Bound"))]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        ["m", "t"].map(|name| ends(door, resumed, f, name))
    });
    let restart = format!("NEWTYPE Meters = Number\nLET Item = {declared}\nEXPR {METERS}");
    let restarted = resumed(&restart, |door| {
        let f = subject(door.root());
        ["m", "t"].map(|name| ends(door, door.load(), f, name))
    });
    assert_eq!(
        under,
        ["[Meters, Meters]", "[Meters, Meters]"].map(String::from)
    );
    assert_eq!(under, restarted);
}

/// `SUBJECT`'s body: `s` calls `f :Fn` by name, and `t` reads `s` alone.
const CALLS: &str = "#(SUBJECT f :Fn) -> Any = #(\n  LET s = (f {x = 1})\n  LET t = s\n  t\n)";

/// A call through a head whose type is a run-bound name is typed by the call a binding gives it:
/// under `Fn := (FN :{x :Number} -> Number)` and `Fn := (FN :{x :Any} -> Str)`, `s` and `t` are
/// what a fresh load with `Fn` declared gives them.
#[test]
fn a_call_through_a_run_bound_head_resumed_is_what_a_fresh_load_types() {
    for declared in [":(FN :{x :Number} -> Number)", ":(FN :{x :Any} -> Str)"] {
        let source = format!(
            "LET Bound = {declared}\nEXPR FOR ALL #{{Fn: :(FN :{{x :Number}} -> Any)}} {CALLS}"
        );
        let under = resumed(&source, |door| {
            let f = subject(door.root());
            let fun = unknown(door.symbols(), f, "Fn");
            let binding = door.binding(&[(fun, door.declared("Bound"))]);
            let resumed =
                (door.resume(None, binding, false)).expect("the resumption refuses nothing");
            ["s", "t"].map(|name| ends(door, resumed, f, name))
        });
        let restart = format!("LET Fn = {declared}\nEXPR {CALLS}");
        let restarted = resumed(&restart, |door| {
            let f = subject(door.root());
            ["s", "t"].map(|name| ends(door, door.load(), f, name))
        });
        assert_eq!(under, restarted, "under `Fn := {declared}`");
    }
}

/// `SUBJECT`'s body: `s` calls the closed `g` by name over `p :Arg`, and `t` reads `s` alone.
const LAYS: &str = "#(SUBJECT p :Arg) -> Any = #(\n  \
                    LET g = (FN :{x :Number} -> Number = #(x))\n  \
                    LET s = (g p)\n  LET t = s\n  t\n)";

/// A call by name whose argument's type is a run-bound name lays it out by the rule a binding
/// closes: under `Arg := {x :Number}`, `s` and `t` are what a fresh load with `Arg` declared gives
/// them, and under `Arg := {x :Number y :Str}`, which names a field `g` does not declare, the
/// resumption refuses as the closed layout does.
#[test]
fn a_call_over_a_run_bound_argument_resumed_is_what_a_fresh_load_lays_out() {
    let source =
        |declared: &str| format!("LET Bound = {declared}\nEXPR FOR ALL #{{Arg: :Any}} {LAYS}");
    let under = resumed(&source(":{x :Number}"), |door| {
        let f = subject(door.root());
        let arg = unknown(door.symbols(), f, "Arg");
        let binding = door.binding(&[(arg, door.declared("Bound"))]);
        let resumed = (door.resume(None, binding, false)).expect("the resumption refuses nothing");
        ["s", "t"].map(|name| ends(door, resumed, f, name))
    });
    let restart = format!("LET Arg = :{{x :Number}}\nEXPR {LAYS}");
    let restarted = resumed(&restart, |door| {
        let f = subject(door.root());
        ["s", "t"].map(|name| ends(door, door.load(), f, name))
    });
    assert_eq!(under, restarted, "under `Arg := {{x :Number}}`");
    let refused = resumed(&source(":{x :Number y :Str}"), |door| {
        let f = subject(door.root());
        let arg = unknown(door.symbols(), f, "Arg");
        let binding = door.binding(&[(arg, door.declared("Bound"))]);
        door.resume(None, binding, false).err()
    });
    let refused = refused.expect("the binding refuses the call");
    assert!(
        refused.contains("can never satisfy the parameters"),
        "{refused}"
    );
}

/// A content tree as a law prints it: whether it is closed, beside its root node's kind. Not its
/// identity: a restart declares a bound name outside the body, where a module body does not
/// capture it, and keys a leaf on a slot it moves.
fn tree_line(tree: &crate::scope::Tree<'_>) -> String {
    use crate::scope::Tree;
    let kind = match tree {
        Tree::Digest(..) => "digest",
        Tree::Body { .. } => "body",
        Tree::Knot { .. } => "knot",
        Tree::View { .. } => "view",
        Tree::Retyped { .. } => "retyped",
        Tree::TypeValue { .. } => "type value",
        Tree::Leaf(_) => "leaf",
    };
    format!("closed {} {kind}", tree.closed())
}

/// Whether a binding of the names `facts`' trees are written over may change one of them: such a
/// record closes by tree substitution, which a walk does, not by a binding's type levels alone.
fn writes_tree_names(types: &Chain<'_, '_, '_, '_>, facts: &crate::scope::UnitFacts<'_>) -> bool {
    (facts.typed.iter()).any(|(_, typed)| {
        (typed.contents.iter()).any(|(_, tree)| writes_names(types, tree))
            || (typed.trees.iter()).any(|(_, tree)| writes_names(types, tree))
    })
}

/// Whether a binding of the names `tree`'s types are written over may change it: a type a node
/// holds written over names, a type value, or a call only an answer selects among.
fn writes_names(types: &Chain<'_, '_, '_, '_>, tree: &crate::scope::Tree<'_>) -> bool {
    use crate::scope::{Leaf, Tree, Within};
    if tree.closed() {
        return false;
    }
    let open = |ty: &crate::type_lattice::Parametric| {
        crate::elaborate::judge::static_of(types, *ty).is_none()
    };
    let own = match tree {
        Tree::Knot { members, .. } => members.iter().any(|node| node.instance.iter().any(open)),
        Tree::View { application, .. } => open(application),
        Tree::Retyped { carried, .. } => open(carried),
        Tree::TypeValue { .. } | Tree::Leaf(Leaf::Call { .. }) => true,
        Tree::Leaf(Leaf::Bound {
            within: Some(Within::Call(call)),
            ..
        }) => call.solution.iter().any(|(_, point)| open(point)),
        _ => false,
    };
    let mut nested = false;
    tree.subtrees(&mut |subtree| nested |= !nested && writes_names(types, subtree));
    own || nested
}

/// A `USING` header over a closed module body, retyped to an application over a run-bound name,
/// picks the body's registrations by questions: whether each covers the header's keyworded member.
/// Its unit holds nothing, and under a binding it is what a fresh load with the name declared reads.
#[test]
fn header_picks_over_a_run_bound_name_are_asked() {
    let body = "#(SUBJECT x :Elt) -> Any = #(\n  \
                LET s0 = (USING (pk :! (Picks WITH {Carrier = Elt})) SCOPE (PICK zero))\n  s0\n)\n";
    let module = "MODULE pk = ((LET zero = 1) (EXPR #(PICK x :Number) -> Str = #(\"n\")))\n";
    let source = format!("{PICKS}{module}EXPR FOR ALL #[Elt] {body}");
    let (holding, under) = resumed(&source, |door| {
        let f = subject(door.root());
        let elt = unknown(door.symbols(), f, "Elt");
        let unit = unit_of(door, f, "s0");
        let holding = (waits_on(door, unit, elt).into_iter().flatten().flatten())
            .filter(|kind| kind.holds())
            .collect::<Vec<_>>();
        let binding = door.binding(&[(elt, KType::NUMBER)]);
        let resumed = (door.resume(None, binding, false)).expect("no refusal");
        (holding, ends(door, resumed, f, "s0"))
    });
    let restarted = resumed(
        &format!("{PICKS}{module}LET Elt = Number\nEXPR {body}"),
        |door| ends(door, door.load(), subject(door.root()), "s0"),
    );
    assert!(holding.is_empty(), "{holding:?}");
    assert_eq!(under, restarted);
}
