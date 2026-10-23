//! What each unit waits on: the unknowns its facts read, transitively, gathered into the unit's
//! frontier; and where it **seams** — the latest bind site among its frontier's unknowns, its own
//! binders aside, as the keys fold reads it.

use crate::program::Program;
use crate::scope::{Address, BindSite, BodyShape, ShapeKind, Slot, UnitId, UnitIndex, Unknown};
use crate::symbols::BinderSymbol;

use super::statics::{body, loaded, slot};
use super::{assert_linear, load_counts};

/// Two overloads of `PICK`, one at `Number` and one at `Str`.
const PICK: &str = "EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                    EXPR #(PICK x :Str) -> Number = #(1)\n";

/// A signature with an unpinned head parameter `Carrier` and a member over it.
const COUNTER: &str = "SIG Counter FOR ALL #[Carrier] = \
                       #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n";

/// The one quantified callable body the program's top level registers.
pub(super) fn quantified<'graph>(program: &Program<'graph>) -> &'graph BodyShape<'graph> {
    let facts = program.facts();
    let mut found = (program.shape().nested_shapes().iter()).filter(|(_, nested)| {
        nested.kind() == ShapeKind::Callable && !facts.group_levels(nested).is_empty()
    });
    let (_, body) = found.next().expect("a quantified registration");
    assert!(found.next().is_none(), "one quantified registration");
    body
}

/// The unit of `shape` that binds `name`.
fn unit_of(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> UnitId {
    UnitId {
        segment: shape.segment(),
        unit: shape.unit_of_slot(slot(program, shape, name)),
    }
}

/// What `unit` waits on.
fn frontier(program: &Program<'_>, unit: UnitId) -> Vec<Unknown> {
    let (_, frontier) = program.facts().unit(unit).expect("the load ran the unit");
    frontier.unknowns().to_vec()
}

/// What the unit binding `name` of `shape` waits on.
fn waits(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> Vec<Unknown> {
    frontier(program, unit_of(program, shape, name))
}

/// Where `unit` seams: the latest bind site among its frontier's unknowns, its own binders aside.
pub(super) fn seam(program: &Program<'_>, unit: UnitId) -> Option<BindSite> {
    let shapes = program.shapes();
    (frontier(program, unit).into_iter())
        .map(|unknown| shapes.bind_site(unknown))
        .filter(|site| (site.segment, site.unit) != (unit.segment, unit.unit))
        .max()
}

/// The entry of `shape`, as a bind site's place.
fn entry_of(shape: &BodyShape<'_>) -> (crate::scope::Segment, UnitIndex) {
    (shape.segment(), UnitIndex::ENTRY)
}

/// Whether some unit of `shape` or of a body nested in it seams at `at` over `unknown`.
fn seams_on(
    program: &Program<'_>,
    shape: &BodyShape<'_>,
    at: impl Fn(BindSite) -> bool,
    unknown: Unknown,
) -> bool {
    let within = |segment| {
        let mut held = Some(segment);
        while let Some(segment) = held {
            if segment == shape.segment() {
                return true;
            }
            held = program.shapes().parent(segment).map(|(parent, _)| parent);
        }
        false
    };
    (program.facts().own_units().iter())
        .filter(|(unit, (_, frontier))| {
            within(unit.segment) && frontier.unknowns().contains(&unknown)
        })
        .any(|(unit, _)| seam(program, *unit).is_some_and(&at))
}

/// The unknown the slot `name` of `shape` is: a type name's or a value binder's.
pub(super) fn unknown(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> Unknown {
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

/// The first body `shape` nests directly of `kind`.
fn nested_of<'graph>(
    shape: &'graph BodyShape<'graph>,
    kind: ShapeKind,
) -> &'graph BodyShape<'graph> {
    let (_, nested) = (shape.nested_shapes().iter())
        .find(|(_, nested)| nested.kind() == kind)
        .unwrap_or_else(|| panic!("a nested {kind:?}"));
    nested
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
    let source = format!(
        "{COUNTER}EXPR FOR ALL #[Elt] #(SAME a :Elt AND b :Elt) -> Elt = #(a)\n\
         LET step = (FN :{{m :Counter}} -> Any = \
         #(LET s = (SAME m.zero AND (m.succ {{x = m.zero}}))))\n"
    );
    loaded(&source, |program| {
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

/// A fellow's read records no edge: a knot's unit waits on what its members read outside the
/// knot, and a knot whose members read only each other waits on nothing.
#[test]
fn a_fellow_read_in_a_knot_waits_on_nothing_the_fellow_reads() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
                  LET a = {next = b, v = x}\n  LET b = {next = a, v = 1}\n  a\n)";
    loaded(source, |program| {
        let f = quantified(program);
        assert_eq!(
            unit_of(program, f, "a"),
            unit_of(program, f, "b"),
            "one knot, one unit"
        );
        assert_eq!(waits(program, f, "a"), [unknown(program, f, "Elt")]);
    });
    let fellows = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
                   LET a = {next = b, v = 1}\n  LET b = {next = a, v = 1}\n  a\n)";
    loaded(fellows, |program| {
        let f = quantified(program);
        assert!(
            waits(program, f, "a").is_empty(),
            "the members read each other alone"
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
        let g = nested_of(f, ShapeKind::Callable);
        assert_eq!(waits(program, g, "p"), [unknown(program, f, "Elt")]);
    });
}

#[test]
fn a_body_over_its_own_for_all_name_seams_at_its_entry() {
    let source =
        format!("{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET p = (PICK x)\n  p\n)");
    loaded(&source, |program| {
        let f = quantified(program);
        let seamed = seam(program, unit_of(program, f, "p")).expect("`p` waits on `Elt`");
        assert_eq!((seamed.segment, seamed.unit), entry_of(f));
    });
}

#[test]
fn a_fact_over_an_unknown_type_binder_seams_at_the_unit_binding_it() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
                  NEWTYPE Boxed = :{v :Elt}\n  LET t = :(LIST OF Boxed)\n  t\n)";
    loaded(source, |program| {
        let f = quantified(program);
        let (elt, boxed) = (unknown(program, f, "Elt"), unknown(program, f, "Boxed"));
        let t = unit_of(program, f, "t");
        let waits = frontier(program, t);
        assert!(waits.contains(&boxed) && waits.contains(&elt), "{waits:?}");
        let seamed = seam(program, t).expect("`t` waits on `Boxed`");
        assert_eq!(
            (seamed.segment, seamed.unit),
            (f.segment(), unit_of(program, f, "Boxed").unit)
        );
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
        let g = nested_of(f, ShapeKind::Callable);
        let p = seam(program, unit_of(program, g, "p")).expect("`p` waits on `Elt`");
        assert_eq!((p.segment, p.unit), entry_of(f), "`p` waits on `Elt` alone");
        let q = unit_of(program, g, "q");
        let key = unknown(program, g, "Key");
        assert!(frontier(program, q).contains(&key));
        let seamed = seam(program, q).expect("`q` waits on `Key`");
        assert_eq!((seamed.segment, seamed.unit), entry_of(g));
    });
}

#[test]
fn a_quote_s_hole_seams_at_its_code_s_entry() {
    loaded("LET q = #(PRINT :(LIST OF Tee))", |program| {
        let code = nested_of(program.shape(), ShapeKind::Code);
        let capture = |unknown: &Unknown| matches!(unknown.at, Address::Capture(_));
        let seamed: Vec<_> = (program.facts().own_units().iter())
            .filter(|(unit, (_, frontier))| {
                unit.segment == code.segment() && frontier.unknowns().iter().any(capture)
            })
            .map(|(unit, _)| seam(program, *unit).expect("a unit over the hole seams"))
            .collect();
        assert!(!seamed.is_empty(), "a unit of the code waits on the hole");
        assert!(
            seamed
                .iter()
                .all(|site| (site.segment, site.unit) == entry_of(code))
        );
    });
}

#[test]
fn a_fact_over_a_local_module_seams_at_the_unit_binding_it() {
    let source = format!(
        "{COUNTER}{PICK}LET step = (FN :{{mk :(FN :{{}} -> Counter)}} -> Any = #(\n  \
         LET n = (mk {{}})\n  LET s = (PICK n.zero)\n  s\n))"
    );
    loaded(&source, |program| {
        let step = body(program, program.shape(), "step");
        let n = unknown(program, step, "n");
        assert!(waits(program, step, "s").contains(&n));
        let n_unit = unit_of(program, step, "n").unit;
        assert!(seams_on(program, step, |site| site.unit == n_unit, n));
    });
}

/// A name a `USING` surfaces from a module parameter is read through the parameter: the unit
/// holding the `USING` binds the surfaced name itself and waits on `m`, a module unknown wherever
/// its content is read, and so seams at the callable's entry.
#[test]
fn a_name_using_surfaces_from_a_parameter_waits_on_the_parameter() {
    let source = format!(
        "{COUNTER}{PICK}LET step = (FN :{{m :Counter}} -> Any = #(\n  \
         LET s = (USING (m :! Counter) SCOPE (PICK zero))\n  s\n))"
    );
    loaded(&source, |program| {
        let step = body(program, program.shape(), "step");
        let s = unit_of(program, step, "s");
        assert_eq!(frontier(program, s), [unknown(program, step, "m")]);
        let seamed = seam(program, s).expect("`s` waits on `m`");
        assert_eq!((seamed.segment, seamed.unit), entry_of(step));
    });
}

#[test]
fn a_quote_s_mark_seams_at_its_code_s_entry() {
    loaded(&format!("{PICK}LET q = #(PICK \\y)"), |program| {
        let code = nested_of(program.shape(), ShapeKind::Code);
        let capture = |unknown: &Unknown| matches!(unknown.at, Address::Capture(_));
        let seamed: Vec<_> = (program.facts().own_units().iter())
            .filter(|(unit, (_, frontier))| {
                unit.segment == code.segment() && frontier.unknowns().iter().any(capture)
            })
            .map(|(unit, _)| seam(program, *unit).expect("a unit over the mark seams"))
            .collect();
        assert!(!seamed.is_empty(), "a unit of the code waits on the mark");
        assert!(
            seamed
                .iter()
                .all(|site| (site.segment, site.unit) == entry_of(code))
        );
    });
}

#[test]
fn a_dollar_name_in_a_quote_waits_on_the_outer_binder_s_type() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET q = #(PICK $x)\n  q\n)"
    );
    loaded(&source, |program| {
        let f = quantified(program);
        let code = nested_of(f, ShapeKind::Code);
        let elt = unknown(program, f, "Elt");
        assert!(
            seams_on(program, code, |_| true, elt),
            "the code's call reads `x` through its outer binder"
        );
        // The call reads its `$` capture too, which binds at the code's entry, past `f`'s.
        assert!(
            seams_on(
                program,
                code,
                |site| (site.segment, site.unit) == entry_of(code),
                elt
            ),
            "the code's unit seams at the later of the two entries"
        );
    });
}

/// A name a `USING` surfaces binds at the unit holding its block, so a callable born in the block
/// waits on it and seams there.
#[test]
fn a_callable_in_a_using_block_waits_on_the_name_it_surfaces() {
    let source = format!(
        "{COUNTER}{PICK}LET step = (FN :{{m :Counter}} -> Any = #(\n  \
         LET s = (USING (m :! Counter) SCOPE (\n    \
         LET g = (FN :{{y :Carrier}} -> Any = #(y))\n    g\n  ))\n  s\n))"
    );
    loaded(&source, |program| {
        let step = body(program, program.shape(), "step");
        let block = nested_of(step, ShapeKind::Block);
        let carrier = unknown(program, block, "Carrier");
        let s = unit_of(program, step, "s");
        assert!(!frontier(program, s).contains(&carrier));
        let g = nested_of(block, ShapeKind::Callable);
        assert!(seams_on(
            program,
            g,
            |site| (site.segment, site.unit) == (s.segment, s.unit),
            carrier
        ));
    });
}

/// A chain of views, each over the one before, lays and notes frontier entries linear in its
/// length: each unit reads the frontier before it as one entry, however many unknowns it holds.
#[test]
fn a_chain_of_reads_lays_and_notes_frontiers_linear_in_its_length() {
    let closed = |n: usize| {
        let mut source = format!(
            "{COUNTER}MODULE ints = ((LET zero = 0) \
             (LET succ = (FN :{{x :Number}} -> Number = #(x + 1))))\n\
             LET v0 = ints\n"
        );
        for at in 1..=n {
            source += &format!("LET v{at} = (v{} :| Counter)\n", at - 1);
        }
        source
    };
    let open = |n: usize| {
        let mut body = String::from("(LET v0 = m)");
        for at in 1..=n {
            body += &format!(" (LET v{at} = (v{} :| Counter))", at - 1);
        }
        format!("{COUNTER}LET f = (FN :{{m :Counter}} -> Any = #({body} (v{n}.zero)))\n")
    };
    let chains: [(&str, &dyn Fn(usize) -> String); 2] = [("closed", &closed), ("open", &open)];
    for (what, program) in chains {
        assert_linear(&format!("{what} frontier entries"), |n| {
            load_counts(&program(n)).frontier_entries
        });
        assert_linear(&format!("{what} unknowns noted"), |n| {
            load_counts(&program(n)).noted
        });
    }
}

/// A call reads its callee's closing cut at the innermost body on the caller's chain enclosing the
/// callee: the callee's own names, off the caller's chain, are past the cut, and a read whose cut
/// keeps nothing is dropped.
#[test]
fn a_call_reads_its_callee_s_closing_cut_to_the_caller_s_chain() {
    let source = "LET pair = FN EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Type = #(Elt)\n\
                  EXPR FOR ALL #[Outer] #(BOTH a :Outer AND b :Outer) -> Type = \
                  #(pair {x = a, y = b})\n\
                  EXPR #(WIDE p :(Number | Str) AND q :(Number | Str)) -> Type = #(BOTH p AND q)";
    loaded(source, |program| {
        let root = program.shape();
        let declaring = |name: &str| {
            (root.nested_shapes().iter())
                .map(|(_, nested)| *nested)
                .find(|nested| {
                    (0..nested.slots()).any(|at| {
                        let named = program
                            .symbols()
                            .display(nested.slot_name(Slot(at as u32)).symbol());
                        named.to_string() == name
                    })
                })
                .expect("a callable declares the name")
        };
        let (both, wide) = (declaring("Outer"), declaring("p"));
        let outer = unknown(program, both, "Outer");
        let facts = program.facts();
        let (_, closed) = facts.closing(both.segment()).expect("`BOTH` closes");
        assert!(
            closed.unknowns().contains(&outer),
            "`BOTH`'s closing waits on `Outer`"
        );
        let closing = facts.closing(wide.segment()).map(|(_, waits)| waits);
        let units = (facts.own_units().iter())
            .filter(|(unit, _)| unit.segment == wide.segment())
            .map(|(_, (_, waits))| *waits);
        for waits in units.chain(closing) {
            assert!(
                !waits.unknowns().contains(&outer),
                "`Outer` is past the cut"
            );
            assert!(waits.reads().iter().all(|cut| cut.keeps_any()));
        }
    });
}
