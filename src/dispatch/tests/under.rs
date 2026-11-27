//! The load's facts over a run-bound name, and a frame's under a binding of it — the load's records
//! answered by the instances the frame's bind sites make ([`Door::under`]) — read through a test
//! door ([`Door`]) after the types' root freezes, and the renderers the stored-question laws share
//! ([`laws`]).

use crate::memory::{Bump, BumpVec, resident};
use crate::parse::ExpressionPart;
use crate::program::{CellSubstrate, Running};
use std::cell::Cell;

use crate::build::lay::BaseInstances;
use crate::memory::Lays;
use crate::scope::{
    Address, Binding, BodyShape, Bound, EncloserWords, Facts, FactsOver, Handles, LoadFacts,
    NameAddress, Payload, RecordSource, Segment, ShapeKind, Slot, Static, Unknown,
};
use crate::symbols::{BinderSymbol, SymbolInterner};
use crate::type_lattice::{KType, display_name};
use crate::values::tests::reading;
use crate::values::{BuildBudget, Chain};

use super::statics::rendered;
use super::{Koan, output};

/// `SIG Counter`, a module signature over a `Carrier` head parameter.
pub(super) const COUNTER: &str = "SIG Counter FOR ALL #[Carrier] = \
                       #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n";

/// What a test reads a load through: the running program, a chain over its frozen root that every
/// read interns through, and the bump a binding is laid in.
pub(super) struct Door<'d, 'graph> {
    running: &'d Running<'graph>,
    pub(super) types: &'d Chain<'d, 'd, 'd, 'd>,
    pub(super) bump: &'d Bump,
}

impl<'d, 'graph: 'd> Door<'d, 'graph> {
    pub(super) fn symbols(&self) -> &'graph SymbolInterner {
        self.running.symbols()
    }

    /// The instances the program's frames make.
    pub(super) fn instances(&self) -> &'graph crate::build::lay::Instances<'graph> {
        self.running.instances()
    }

    pub(super) fn root(&self) -> &'graph BodyShape<'graph> {
        self.running.program().shapes().root()
    }

    pub(super) fn load(&self) -> &'graph LoadFacts<'graph> {
        self.running.program().facts()
    }

    /// `staged` laid down as a binding.
    pub(super) fn bound(&self, staged: &[(Unknown, Bound<'d>)]) -> Binding<'d> {
        let mut held = BumpVec::new_in(self.bump);
        held.extend_from_slice(staged);
        Binding::laid(self.bump, &mut held)
    }

    /// What a module unknown binds to: the top-level module binder `name`, its exact type over the
    /// closed content tree the load read for it.
    pub(super) fn module(&self, name: &str) -> Bound<'d> {
        let (root, load) = (self.root(), self.load());
        let at = slot(self.symbols(), root, name);
        let Some(crate::scope::Holding::Type(typed)) = load.binder_type(self.types, root, at)
        else {
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

    /// What a frame of `shape` reads once its slots bind `staged`: the load's records answered by
    /// the instances its bind sites, and those of each frame around it, make where `staged` binds
    /// every name a site reads, and the binding written into the records' types. The first
    /// refusal an instance answers, worded under the binding as the frame faults with it.
    pub(super) fn under(
        &self,
        shape: &BodyShape<'_>,
        staged: &[(Unknown, Bound<'d>)],
    ) -> Result<Facts<'d>, String> {
        match self.framed(shape, staged) {
            (facts, None) => Ok(facts),
            (_, Some(refusal)) => Err(refusal),
        }
    }

    /// [`Door::under`]'s facts beside its refusal, which leaves them readable.
    pub(super) fn framed(
        &self,
        shape: &BodyShape<'_>,
        staged: &[(Unknown, Bound<'d>)],
    ) -> (Facts<'d>, Option<String>) {
        let instances = self.running.instances();
        let laid = instances.binding;
        let binding = self.bound(staged);
        let segment_of = |body| {
            let at = (laid.facts.bodies.iter()).position(|held| *held == Some(body));
            Segment(at.expect("a graph body is a shape's") as u32)
        };
        let body = (laid
            .facts
            .bodies
            .get(shape.segment().index() as usize)
            .copied())
        .flatten()
        .expect("the body is one of the graph's");
        // Each bind site of a body whose names the binding binds makes its instance into the
        // body's slab, in site order, as a frame runs through them.
        let mut refused = None;
        let mut slab = |body: bindgraph::BodyId| -> &'d [Cell<u64>] {
            let slab = self.bump.fill(laid.layout.width(body), |_| Cell::new(0));
            let nodes = (laid.graph.nodes().iter())
                .filter(|node| node.site().body() == body)
                .filter_map(|node| laid.graph.node_at(node.site()));
            for node in nodes {
                let mut bound = Vec::new();
                let open = laid.graph.relevant(node).iter().any(|name| {
                    let at = match laid.facts.name(*name).address {
                        NameAddress::Site(unknown) => binding.bound(unknown),
                        _ => None,
                    };
                    if let Some(at) = at {
                        bound.push((*name, at));
                    }
                    at.is_none() && !laid.graph.is_answered(*name)
                });
                if open || laid.graph.node(node).handle_width() == 0 {
                    continue;
                }
                let range = laid.layout.range(laid.graph, node);
                let mut words: Vec<u64> = slab[range.clone()].iter().map(Cell::get).collect();
                let made = instances.make(node, &bound, self.types, self.bump, &mut words);
                refused = refused.or(made);
                (slab[range].iter().zip(words)).for_each(|(cell, word)| cell.set(word));
                slab[laid.layout.made(node)].set(1);
            }
            slab
        };
        let own = slab(body);
        let enclosers =
            std::iter::successors(laid.graph.encloser(body), |body| laid.graph.encloser(*body))
                .map(|encloser| {
                    let words = slab(encloser).iter().map(Cell::get);
                    EncloserWords {
                        body: segment_of(encloser),
                        words: crate::memory::collect(
                            self.bump,
                            words.collect::<Vec<_>>().into_iter(),
                        ),
                    }
                });
        let enclosers =
            crate::memory::collect(self.bump, enclosers.collect::<Vec<_>>().into_iter());
        let handles = Handles {
            slab: own,
            enclosers,
        };
        let facts = (Facts::under(self.load()).answering(instances))
            .rebinding(instances)
            .framed(handles, Some(binding));
        let Some((question, arm)) = refused else {
            return (facts, None);
        };
        let stored = laid.facts.question(question).stored();
        let Some(Payload::Refusal(refusal)) = stored.map(|stored| stored.arms[usize::from(arm)])
        else {
            panic!("a refused answer names its refusal's template");
        };
        let refusal = facts.bound_refusal(self.types, self.bump, shape.segment(), refusal);
        let refusal = refusal.display(self.symbols(), self.types).to_string();
        (facts, Some(refusal))
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

/// Load and run `source`, then hand `check` the door its reads go through: `None` where the
/// program does not load.
pub(super) fn ran<R>(
    source: &str,
    check: impl for<'d, 'graph> FnOnce(&Door<'d, 'graph>) -> R,
) -> Option<R> {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output()).ok()?;
    Some(substrate.with(|running| {
        // A run that faults keeps the instances its frames made before the fault.
        let _ = running.ran();
        let bump = Bump::new();
        let types = reading(running.program().tiers(), &bump);
        check(&Door {
            running,
            types: &types,
            bump: &bump,
        })
    }))
}

/// Load `source` and hand `check` the door its reads go through.
pub(super) fn opened<R>(
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
pub(super) fn slot(symbols: &SymbolInterner, shape: &BodyShape<'_>, name: &str) -> Slot {
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

/// `SUBJECT`'s body: the last callable the top level births.
pub(super) fn subject<'graph>(root: &'graph BodyShape<'graph>) -> &'graph BodyShape<'graph> {
    (root.nested_shapes().iter().rev())
        .find(|(_, nested)| nested.kind() == ShapeKind::Callable)
        .map(|(_, nested)| *nested)
        .expect("the program registers `SUBJECT`")
}

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
pub(super) fn waits_on(
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

/// `Pick`, `Named`, and `pk`, which is named and whose overloads `PICK` a number or a string.
const PK: &str = r#"SIG Pick FOR ALL #[Carrier] = #[(EXPR #(PICK x :Carrier) -> Str)]
SIG Named = #[(VAL name :Str)]
MODULE pk = (
  LET name = "pk"
  EXPR #(PICK x :Number) -> Str = #("n")
  EXPR #(PICK x :Str) -> Str = #("s")
)
"#;

/// `SUBJECT`'s body: a header over `pk` at a meet the lattice cannot spell over `Elt`.
const MET_HEADER: &str = r#"#(SUBJECT x :Elt) -> Any = #(
  USING (pk :! ((Pick WITH {Carrier = Elt}) & Named)) SCOPE (
    PICK x
  )
)
"#;

/// A registration over a name whose bound already misses every builtin overload at its key never
/// overlaps one: the check asks nothing.
#[test]
fn a_registration_whose_bound_misses_the_builtins_asks_nothing() {
    let source = "EXPR FOR ALL #{Elt: Str} #(SUBJECT x :Elt) -> Any = #(\n  \
                  OP #(+) OVER Elt = #(x)\n  x\n)";
    let marked = opened(source, |door| {
        (door.load().own_units()).any(|(_, (_, frontier))| {
            (frontier.marks().iter()).any(|noted| {
                matches!(noted.mark, crate::scope::Mark::Open(open)
                    if open.kind == crate::scope::OpenKind::Overlap)
            })
        })
    });
    assert!(!marked, "no unit is marked for an overlap");
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

/// A field read of a variable whose bound guarantees the field names the variable and judges
/// nothing over it, so a binding absorbs the unit; over a variable bounded by a signature
/// declaring the member `ATTR` reads the binding's module's member: a question, which holds
/// nothing. A bound declaring no such member refuses the read at load.
#[test]
fn a_field_read_through_a_guaranteeing_bound_names_its_variable() {
    let marks = |group: &str| {
        let source = format!(
            "SIG Has = #[(VAL f :Number)]\n\
             EXPR FOR ALL {group} #(GET x :Elt) -> Any = #(\n  LET y = x.f\n  y\n)"
        );
        opened(&source, |door| {
            let f = quantified(door);
            let elt = unknown(door.symbols(), f, "Elt");
            let y = unit_of(door, f, "y");
            waits_on(door, y, elt).unwrap_or_default()
        })
    };
    let signed = marks("#{Elt: Has}");
    assert!(
        signed.contains(&Some(crate::scope::OpenKind::Spells)),
        "{signed:?}"
    );
    let refusal =
        super::run("EXPR FOR ALL #{Elt: Module} #(GET x :Elt) -> Any = #(\n  LET y = x.f\n  y\n)");
    assert!(
        refusal.contains("Elt has no field f: its bound Module declares none"),
        "{refusal}"
    );
}

/// What the generator's `BuiltModule` binds is content whose tree holds a leaf no binder names: a
/// call of a registration building a module over a record literal.
#[test]
fn a_module_built_over_a_record_literal_holds_a_leaf_no_binder_names() {
    let source = r#"EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(
  EXPR #(MKB r :{x :Number}) -> Module = #(
    (MODULE built OVER #[r] = ((LET Zero = Number) (LET zero = r.x)))
    built
  )
  LET hq = (MKB {x = 1})
  x
)
"#;
    let identity = opened(source, |door| {
        let f = subject(door.root());
        let tree = (door.load().content(f, slot(door.symbols(), f, "hq")))
            .expect("`hq` holds a content tree");
        tree.identity()
    });
    assert_eq!(identity, crate::scope::Identity::Unnamed);
}

/// A header whose carried type spells no application, a meet over a run-bound name, records no
/// picks: the run picks the covering overloads itself, which are what a fresh load with the name
/// declared offers. Its units hold nothing.
#[test]
fn header_picks_over_a_carried_type_no_application_spells_are_a_fresh_load_s() {
    let source = format!("{PK}EXPR FOR ALL #[Elt] {MET_HEADER}");
    let carried = opened(&source, |door| {
        let f = quantified(door);
        let expressions = f.type_expressions();
        let met = expressions
            .iter()
            .max_by_key(|expression| expression.site)
            .expect("the header writes a type");

        match door.load().typed_expression(door.types, f, met.site) {
            Static::Rigid { value, .. } => {
                display_name(value, door.types, door.symbols()).to_string()
            }
            other => panic!("the meet is written over `Elt`: {other:?}"),
        }
    });
    assert!(carried.starts_with('('), "a derived name: {carried}");
    for (call, declared) in [("\"a\"", "Str"), ("1", "Number")] {
        let ran = super::run(&format!("{source}PRINT (SUBJECT {call})"));
        let restart = format!("{PK}LET Elt = :{declared}\nEXPR {MET_HEADER}PRINT (SUBJECT {call})");
        // A header at a meet stalls the run today, closed or not; the two stall alike.
        assert_eq!(ran, super::run(&restart), "under `Elt := {declared}`");
    }
}

/// The narrowing of the use `LET name = <use>` in `shape`, as `facts` holds it.
pub(super) fn narrowing<'a, S: RecordSource<'a>>(
    door: &Door<'_, '_>,
    facts: &FactsOver<S>,
    shape: &BodyShape<'_>,
    name: &str,
) -> String {
    let Some(ExpressionPart::Expression(node)) = shape.rhs(slot(door.symbols(), shape, name))
    else {
        panic!("`{name}` is bound to a node");
    };
    let site = shape.site_of_node(node);
    rendered((facts.use_statics(door.types, door.bump, shape, site)).narrowing)
}

/// The ends of the binder `name` of `shape`, as `facts` holds them, rendered.
pub(super) fn ends<'a, S: RecordSource<'a>>(
    door: &Door<'_, '_>,
    facts: &FactsOver<S>,
    shape: &BodyShape<'_>,
    name: &str,
) -> String {
    let at = slot(door.symbols(), shape, name);
    let Some(crate::scope::Holding::Type(typed)) = facts.binder_type(door.types, shape, at) else {
        panic!("`{name}` is typed, and no scheme");
    };
    let render = |ty| display_name(ty, door.types, door.symbols()).to_string();
    format!("[{}, {}]", render(typed.lower), render(typed.upper))
}

/// What a fresh load of `restart`, a program with a binding written as a declaration, reads by
/// `read`: where it declares a name `Any`, each parameter declared at that name is plain `Any`, as
/// a frame whose binding makes a `FOR ALL` name `Any` reads it ([`unnamed_any`]).
pub(super) fn restarted_by<R>(restart: &str, read: impl FnOnce(&Door<'_, '_>) -> R) -> R {
    match restart.contains(" = :Any\n") {
        true => crate::dispatch::walk::unnamed_any(|| opened(restart, read)),
        false => opened(restart, read),
    }
}

/// Two overloads of `PICK`, one at `Number` and one at `Str`.
const PICK: &str = "EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                    EXPR #(PICK x :Str) -> Number = #(1)\n";

/// A use the load keeps two candidates of over `Elt` is selected by the instance a binding of
/// `Elt` makes: the `Number` overload, which returns `Str`.
#[test]
fn binding_a_for_all_name_selects_the_overload_at_its_type() {
    let source =
        format!("{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET p = (PICK x)\n  p\n)");
    let (load, under) = opened(&source, |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let under = (door.under(f, &[(elt, Bound::Type(KType::NUMBER))])).expect("no refusal");
        let read = |facts: &Facts<'_>| (narrowing(door, facts, f, "p"), ends(door, facts, f, "p"));
        (narrowing(door, door.load(), f, "p"), read(&under))
    });
    assert_eq!(load, "kept maybe maybe");
    assert_eq!(under, ("selected".to_string(), "[Str, Str]".to_string()));
}

/// Two `FIRST` overloads: one over a record whose field `x` is `Elt`, one at `Number`.
const FIRSTS: &str = "EXPR FOR ALL #[Elt] #(FIRST r :{x :Elt}) -> Elt = #(r.x)\n\
                      EXPR #(FIRST n :Number) -> Number = #(n)\n";

/// `SUBJECT`'s body: `s` calls `FIRST` over `y :Item`, and `t` reads `s` alone.
const SELECTS: &str = "#(SUBJECT y :Item) -> Any = #(\n  LET s = (FIRST y)\n  LET t = s\n  t\n)";

/// A call keeping two candidates over `y :Item` is typed by which runs: the instance a binding of
/// `Item` makes gives `s` and `t` what a fresh load with `Item` declared selects — the record
/// overload under a record, the `Number` one under `Number`, and both at most under `Any`, where
/// the call reads the carried type.
#[test]
fn a_call_s_type_under_a_binding_is_what_a_fresh_load_selects() {
    for declared in [":{x :Str}", ":Number", ":Any"] {
        let source = format!("LET Bound = {declared}\n{FIRSTS}EXPR FOR ALL #[Item] {SELECTS}");
        let under = opened(&source, |door| {
            let f = subject(door.root());
            let item = unknown(door.symbols(), f, "Item");
            let bound = Bound::Type(door.declared("Bound"));
            let under = (door.under(f, &[(item, bound)])).expect("no refusal");
            ["s", "t"].map(|name| ends(door, &under, f, name))
        });
        let restart = format!("{FIRSTS}LET Item = {declared}\nEXPR {SELECTS}");
        let restarted = restarted_by(&restart, |door| {
            let f = subject(door.root());
            ["s", "t"].map(|name| ends(door, door.load(), f, name))
        });
        assert_eq!(under, restarted, "under `Item := {declared}`");
    }
}

/// The renderers the stored-question laws read a load's facts by, each fact rendered by names —
/// never by an index or a source position — and the generated programs' run law.
pub(super) mod laws {
    use proptest::prelude::*;

    use crate::scope::{
        Address, BodyShape, Bound as Bind, CandidateList, Narrowing, Slot, Target, Tree, Unknown,
    };
    use crate::symbols::{BinderSymbol, SymbolInterner};
    use crate::type_lattice::KType;

    use super::super::generate::{Bound, MODULES, Resumable, enclosed_bound};
    use super::{Door, subject};

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
        door.bound(&staged(door, bound, false))
    }

    /// [`binding`]'s entries, each module bound to its content tree's digest beside the module as
    /// a view over it reads it, as a frame binds the module its slot holds where `by_digest`: a
    /// `MODULE` body is a transparent view of itself at the signature it carries.
    pub(in super::super) fn staged<'d>(
        door: &Door<'d, '_>,
        bound: &Bound,
        by_digest: bool,
    ) -> Vec<(Unknown, Bind<'d>)> {
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
                MODULES
                    .contains(&name)
                    .then(|| door.module(&Resumable::stamped(name))),
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
        staged
    }

    /// A narrowing by each candidate's index on its use's list.
    pub(in super::super) fn narrowed(narrowing: Narrowing<'_>, list: &CandidateList<'_>) -> String {
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

    /// A refusal's message, its location dropped, and the place a repeated guard names its first
    /// written at: a restart writes its binding as statements of its own, which move every place
    /// past them.
    pub(in super::super) fn unlocated(message: &str) -> String {
        let message = match message.split_once(": ") {
            Some((at, rest)) if at.starts_with('<') => rest,
            _ => message,
        };
        canonical(
            message
                .split_once("; first at ")
                .map_or(message, |(repeat, _)| repeat),
        )
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

    /// The levels `bound` gives a type, read off `SUBJECT`'s homes.
    pub(in super::super) fn levels(door: &Door<'_, '_>, bound: &Bound) -> Vec<(usize, KType)> {
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

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: crate::tests::case_share(1, 4),
            ..ProptestConfig::default()
        })]

        /// Outputs agree: where a load with `Elt` declared runs, a program whose callable's bound
        /// names `Elt` prints what it prints, the questions over the bound as `Elt`'s binding
        /// answers them selecting the callable as the load selects it by the closed bound; where
        /// that load refuses a held `FN` the program calls, the program is refused. The call solves
        /// against the scheme the value is born with, so the scheme born is the bound closed under
        /// `Elt`'s binding.
        #[test]
        fn a_callable_bounded_over_an_encloser_name_prints_as_the_fresh_load(case in enclosed_bound()) {
            // What a run prints and the fault it ends in, its trace frames aside: they name the
            // encloser, whose head the restart writes otherwise.
            let printed = |source: &str| -> Vec<String> {
                (super::super::run(source).lines())
                    .filter(|line| !line.starts_with("  in "))
                    .map(str::to_string)
                    .collect()
            };
            let restarted = printed(&case.restart());
            let rendered = printed(&case.render());
            // A load with `Elt` declared that refuses the program is refused where the program
            // runs the code refused: a held callable's call always runs, and its born scheme
            // admits only what the closed bound admits. A refusal of the call under `Elt`'s
            // binding is in the load's words. One under every binding is the load's, which names
            // `Elt` open; one of the callable's body is its entry's, raised as the call enters
            // it, at `Tee`'s solution.
            let refused = restarted.first().is_some_and(|line| line.starts_with("load: "));
            prop_assume!(!refused || case.held);
            let restart = case.restart();
            let call = (restart.lines()).position(|line| line.trim_start().starts_with("inner {"));
            let at_call = call.is_some_and(|index| {
                restarted[0].starts_with(&format!("load: <test>:{}:", index + 1))
            }) && rendered.first().is_some_and(|line| line.starts_with("error: "));
            match refused {
                true if at_call => {
                    use super::super::graphs::printed_unlocated;
                    prop_assert_eq!(
                        printed_unlocated(&case.render()),
                        printed_unlocated(&restart),
                        "in\n{}\nrestarted as\n{}",
                        case.render(),
                        restart
                    )
                }
                true => prop_assert!(
                    rendered.iter().any(|line| line.starts_with("load: ") || line.starts_with("error: ")),
                    "in\n{}\nprinted\n{:?}\nwhere\n{}\nis refused",
                    case.render(),
                    rendered,
                    restart
                ),
                // A fault's source position aside: the restart declares `Elt` on a line of its own.
                false => {
                    let unplaced = |lines: &[String]| -> Vec<String> {
                        (lines.iter())
                            .map(|line| match line.split_once("<test>:") {
                                Some((before, after)) => {
                                    let rest = after.splitn(3, ':').nth(2).unwrap_or(after);
                                    format!("{before}{}", rest.trim_start())
                                }
                                None => line.clone(),
                            })
                            .collect()
                    };
                    prop_assert_eq!(unplaced(&rendered), unplaced(&restarted), "in\n{}\nrestarted as\n{}", case.render(), case.restart())
                }
            }
        }
    }

    /// The stored laws draw a `:Module` parameter, and one declared at a meet, that a binding
    /// binds and a statement reads: on a fixed seed, one of 128 cases does, for each.
    #[test]
    fn the_stored_laws_draw_a_bound_module_parameter() {
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
}
