//! What each value channel fact waits on: the unknowns its rule read, transitively, recorded in
//! its shape's residual where there are any.

use crate::program::Program;
use crate::scope::{Address, BodyShape, FactKey, SeamAt, ShapeKind, Slot, Unknown, frontier_at};
use crate::symbols::BinderSymbol;

use super::statics::{body, loaded, slot};

/// Two overloads of `PICK`, one at `Number` and one at `Str`.
const PICK: &str = "EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                    EXPR #(PICK x :Str) -> Number = #(1)\n";

/// The one quantified callable body the program's top level registers.
fn quantified<'graph>(program: &Program<'graph>) -> &'graph BodyShape<'graph> {
    let mut found = (program.shape().nested_shapes().iter()).filter(|(_, nested)| {
        nested.kind() == ShapeKind::Callable && !nested.group_levels().is_empty()
    });
    let (_, body) = found.next().expect("a quantified registration");
    assert!(found.next().is_none(), "one quantified registration");
    body
}

/// What the binder `name` of `shape` waits on.
fn waits(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> Vec<Unknown> {
    let at = FactKey::Slot(slot(program, shape, name));
    frontier_at(shape.statics_residual(), at)
        .unknowns()
        .to_vec()
}

/// The unknown the slot `name` of `shape` is: a type name's or a value binder's.
fn unknown(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> Unknown {
    let found = (0..shape.slots()).map(|at| Slot(at as u32)).find(|at| {
        let named = shape.slot_name(*at);
        !matches!(named, BinderSymbol::Registration(_))
            && program.symbols().display(named.symbol()).to_string() == name
    });
    Unknown {
        segment: shape.segment(),
        at: Address::Slot(found.unwrap_or_else(|| panic!("`{name}` is bound here"))),
    }
}

#[test]
fn a_call_over_a_for_all_name_against_closed_overloads_waits_on_it() {
    let source =
        format!("{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET p = (PICK x)\n  p\n)");
    loaded(&source, |program| {
        let f = quantified(program);
        assert_eq!(waits(program, f, "p"), [unknown(program, f, "Elt")]);
    });
    let closed =
        format!("{PICK}LET f = (FN :{{x :Number}} -> Any = #(\n  LET p = (PICK x)\n  p\n))");
    loaded(&closed, |program| {
        let f = body(program, program.shape(), "f");
        assert!(
            waits(program, f, "p").is_empty(),
            "a call over closed types is fixed"
        );
    });
}

#[test]
fn a_path_through_a_module_parameter_waits_on_the_parameter() {
    let source = "SIG Counter FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                  EXPR FOR ALL #[Elt] #(SAME a :Elt AND b :Elt) -> Elt = #(a)\n\
                  LET step = (FN :{m :Counter} -> Any = \
                  #(LET s = (SAME m.zero AND (m.succ {x = m.zero}))))\n";
    loaded(source, |program| {
        let step = body(program, program.shape(), "step");
        assert!(waits(program, step, "s").contains(&unknown(program, step, "m")));
    });
}

#[test]
fn an_eval_s_declared_type_is_fixed_however_wide() {
    let source = "LET q = #(1)\nEXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
                  LET y = (EVAL q -> Any)\n  y\n)";
    loaded(source, |program| {
        let f = quantified(program);
        assert!(waits(program, f, "y").is_empty());
    });
}

#[test]
fn a_fellow_read_in_a_knot_waits_on_nothing_the_fellow_reads() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
                  LET a = {next = b, v = x}\n  LET b = {next = a, v = 1}\n  a\n)";
    loaded(source, |program| {
        let f = quantified(program);
        assert_eq!(waits(program, f, "a"), [unknown(program, f, "Elt")]);
        assert!(
            waits(program, f, "b").is_empty(),
            "`b` reads `a`'s seed alone"
        );
    });
}

#[test]
fn a_fact_in_an_inner_body_over_an_outer_name_waits_on_the_outer_slot() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
         LET g = (FN :{{}} -> Any = #(\n    LET p = (PICK x)\n    p\n  ))\n  g\n)"
    );
    loaded(&source, |program| {
        let f = quantified(program);
        let (_, g) = (f.nested_shapes().iter())
            .find(|(_, nested)| nested.kind() == ShapeKind::Callable)
            .expect("`g` is nested in `f`");
        assert_eq!(waits(program, g, "p"), [unknown(program, f, "Elt")]);
    });
}

/// `shape`'s seams, each rendered as where it lies beside how many unknowns it waits on.
fn seams(shape: &BodyShape<'_>) -> Vec<(SeamAt, Vec<Unknown>)> {
    (shape.seams().iter())
        .map(|seam| (seam.at, seam.waits_on.unknowns().to_vec()))
        .collect()
}

#[test]
fn a_body_over_its_own_for_all_name_seams_at_its_entry() {
    let source =
        format!("{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET p = (PICK x)\n  p\n)");
    loaded(&source, |program| {
        let f = quantified(program);
        assert_eq!(
            seams(f),
            [(SeamAt::Entry, vec![unknown(program, f, "Elt")])]
        );
        assert!(f.reach().contains(unknown(program, f, "Elt")));
    });
}

#[test]
fn a_fact_over_an_unknown_type_binder_seams_at_the_unit_binding_it() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
                  NEWTYPE Boxed = :{v :Elt}\n  LET t = :(LIST OF Boxed)\n  t\n)";
    loaded(source, |program| {
        let f = quantified(program);
        let (elt, boxed) = (unknown(program, f, "Elt"), unknown(program, f, "Boxed"));
        let listed = seams(f);
        let unit = listed
            .iter()
            .find(|(at, _)| matches!(at, SeamAt::Unit(_)))
            .expect("a unit seam");
        assert!(unit.1.contains(&boxed) && unit.1.contains(&elt));
    });
}

#[test]
fn an_inner_body_s_facts_seam_where_their_last_member_binds() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
         EXPR FOR ALL #[Key] #(INNER k :Key) -> Any = #(\n    \
         LET p = (PICK x)\n    LET q = [k, x]\n    p\n  )\n  x\n)"
    );
    loaded(&source, |program| {
        let f = quantified(program);
        let (_, g) = (f.nested_shapes().iter())
            .find(|(_, nested)| nested.kind() == ShapeKind::Callable)
            .expect("`g` is nested in `f`");
        let (elt, key) = (unknown(program, f, "Elt"), unknown(program, g, "Key"));
        assert!(
            seams(f).contains(&(SeamAt::Entry, vec![elt])),
            "`p` waits on `Elt` alone"
        );
        let inner = seams(g);
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0].0, SeamAt::Entry);
        assert!(inner[0].1.contains(&key) && inner[0].1.contains(&elt));
        assert!(
            f.reach().contains(key),
            "a body's reach holds its nested bodies'"
        );
    });
}

#[test]
fn a_quote_s_hole_seams_at_its_code_s_entry() {
    loaded("LET q = #(PRINT :(LIST OF Tee))", |program| {
        let (_, code) = (program.shape().nested_shapes().iter())
            .find(|(_, nested)| nested.kind() == ShapeKind::Code)
            .expect("the quote's code");
        let listed = seams(code);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, SeamAt::Entry);
        assert!(matches!(
            listed[0].1[..],
            [Unknown {
                at: Address::Capture(_),
                ..
            }]
        ));
    });
}
