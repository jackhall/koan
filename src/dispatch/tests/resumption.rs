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
    inspect: impl for<'g, 'l, 'reg, 'door> FnOnce(&Loaded<'g, 'l, 'reg, 'door, Koan>) -> R,
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
fn upper<'g, 'l, 'reg, 'door>(
    loaded: &Loaded<'g, 'l, 'reg, 'door, Koan>,
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
fn binding_a_name_no_fact_reads_re_runs_nothing() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt Key] #(WRAP x :Elt) -> Any = #(\n  LET p = (PICK x)\n  p\n)"
    );
    let (reran, settled_any) = resumed(&source, |loaded| {
        let f = quantified(loaded.shape());
        let binding = loaded.binding(&[(unknown(loaded.symbols(), f, "Key"), KType::NUMBER)]);
        let settled = loaded
            .resume(&[], binding, false)
            .expect("the resumption refuses nothing");
        (settled.reran, !settled.statics.is_empty())
    });
    assert_eq!(reran, (0, 0));
    assert!(!settled_any, "no shape is walked");
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
