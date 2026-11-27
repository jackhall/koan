//! A frame's facts under a binding of a module binder, or of a name a module's paths, views,
//! content trees or headers are written over: each read through [`Door::under`], the load's records
//! answered by the instances the frame's bind sites make, and set beside what the load holds and
//! what a fresh load, with the binding written as a declaration, reads.

use crate::memory::{Bump, resident};
use crate::parse::ExpressionPart;
use crate::program::CellSubstrate;
use crate::scope::{
    Address, Bare, BodyShape, Bound, CaptureSlot, FactsOver, Identity, RecordSource, ShapeKind,
    Site, Static, Target, Tree, Unknown,
};
use crate::symbols::SymbolInterner;
use crate::type_lattice::{DeclaredType, Interval, KType, Parametric, Variable, display_name};
use crate::values::BuildBudget;

use super::under::{
    COUNTER, Door, ends, laws::unlocated, narrowing, opened, quantified, slot, subject, unit_of,
    unknown,
};
use super::{Koan, output};

/// Two overloads of `PICK`, one at `Number` and one at `Str`.
const PICK: &str = "EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                    EXPR #(PICK x :Str) -> Number = #(1)\n";

/// `ints`, a module carrying `Number`, and `stamped`, it at `Counter`.
const STAMPED_COUNTER: &str = "MODULE ints = ((LET Carrier = Number) (LET zero = 0) \
                               (LET succ = (FN :{x :Number} -> Number = #(x))))\n\
                               LET stamped :Counter = ints\n";

/// `ints`, a module carrying `Number`, and `pinned`, it at `Counter` with its carrier pinned.
const PINNED: &str = "MODULE ints = ((LET Carrier = Number) (LET zero = 0) \
                      (LET succ = (FN :{x :Number} -> Number = #(x))))\n\
                      LET pinned :(Counter WITH {Carrier = Number}) = ints\n";

/// `bound`, a module carrying `Number`, and `stamped`, it at `Counter`.
const BOUND_COUNTER: &str = "MODULE bound = ((LET Carrier = Number) (LET zero = 0) \
                             (LET succ = (FN :{x :Number} -> Number = #(x))))\n\
                             LET stamped :Counter = bound\n";

/// `Counter` and `ints`, which carries no `Carrier` of its own, beside `USE`, whose code reads
/// `$r.zero`.
const USE: &str = "SIG Counter FOR ALL #[Carrier] = \
                   #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                   MODULE ints = ((LET zero = 0) \
                   (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n\
                   EXPR #(USE r :Counter) -> Any = #(EVAL #($r.zero) -> Any)\n";

/// `Picks`, a signature over a `Carrier` whose `PICK` head a `USING` surfaces.
const PICKS: &str = "SIG Picks FOR ALL #[Carrier] = \
                     #[(VAL zero :Carrier) (EXPR #(PICK x :Carrier) -> Str)]\n";

/// `Paired`, and a module stamped at it with `Left = Number`.
const PAIRED: &str = "SIG Paired FOR ALL #[Left Right] = #[(VAL left :Left) (VAL right :Right)]\n\
                      MODULE pair = ((LET left = 1) (LET right = \"r\"))\n\
                      LET stamped :(Paired WITH {Left = Number}) = pair\n";

/// `Counter`, and `MKU`, whose overloads build a module over a number or a string, neither of
/// which `Counter` admits; `made` is a module of the shape `MKU 1` builds, closed at load.
const MKU: &str = r#"SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]
EXPR #(MKU n :Number) -> Module = #(
  MODULE built OVER #[n] = (
    LET Zero = Number
    LET zero = n
  )
  built
)
EXPR #(MKU s :Str) -> Module = #(
  MODULE built OVER #[s] = (
    LET Zero = Str
    LET zero = s
  )
  built
)
MODULE made = (
  LET Zero = Number
  LET zero = 1
)
"#;

/// `bound`, a module carrying `Number` beside a one-parameter family `Wrap`, and `stamped`, it at
/// `:Module`, which a binding of a `:Module` parameter gives.
const STAMPED: &str = "MODULE bound = ((LET Carrier = Number) (NEWTYPE (Type -> Wrap)) (LET zero = 0))\n\
                       LET stamped :Module = bound\n";

/// The signature and the module `WRAP`'s `c` is bound to.
const WRAPS: &str = "SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
     MODULE ints = (LET zero = 0)\n\
     LET held :Counter = ints\n";

/// What a frame binds a module binder to when its slot holds the top-level module `name`: the
/// digest of the content `source`, the module `name` is declared over, beside its bare signature,
/// retyped where `name` is a retype of it.
fn framed<'d>(door: &Door<'d, '_>, name: &str, source: &str) -> Bound<'d> {
    let Bound::Module { carried, tree } = door.module(name) else {
        unreachable!("a module binds a module")
    };
    let Bound::Module {
        carried: own,
        tree: content,
    } = door.module(source)
    else {
        unreachable!("a module binds a module")
    };
    let bare = matches!(content, Tree::Body { .. }).then(|| {
        resident(
            door.bump,
            Bare {
                signature: Some(own),
                sealing: None,
            },
        )
    });
    let digest = Tree::Digest(content.digest().expect("a closed tree"), bare);
    let tree = match tree {
        Tree::Retyped { .. } => Tree::retyped_closed(carried, resident(door.bump, digest)),
        _ => digest,
    };
    Bound::Module {
        carried,
        tree: resident(door.bump, tree),
    }
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

/// The upper end of the binder `name` of `shape`, as `facts` holds it, rendered.
fn upper<'a, S: RecordSource<'a>>(
    door: &Door<'_, '_>,
    facts: &FactsOver<S>,
    shape: &BodyShape<'_>,
    name: &str,
) -> String {
    let at = slot(door.symbols(), shape, name);
    let Some(crate::scope::Holding::Type(typed)) = facts.binder_type(door.types, shape, at) else {
        panic!("`{name}` is typed, and no scheme");
    };
    display_name(typed.upper, door.types, door.symbols()).to_string()
}

/// Whether the type binder `name` of `shape` declares closed, as `facts` holds it.
fn declares_closed<'a, S: RecordSource<'a>>(
    door: &Door<'_, '_>,
    facts: &FactsOver<S>,
    shape: &BodyShape<'_>,
    name: &str,
) -> bool {
    let at = slot(door.symbols(), shape, name);
    matches!(
        facts.declared_type(door.types, shape, at),
        Static::Closed(_)
    )
}

/// The type the type binder `name` of `shape` declares, as `facts` holds it, rendered: `None`
/// where it is unknown.
fn declared_as<'a, S: RecordSource<'a>>(
    door: &Door<'_, '_>,
    facts: &FactsOver<S>,
    shape: &BodyShape<'_>,
    name: &str,
) -> Option<String> {
    let at = slot(door.symbols(), shape, name);
    match facts.declared_type(door.types, shape, at) {
        Static::Unknown => None,
        Static::Closed(ty) => Some(display_name(ty, door.types, door.symbols()).to_string()),
        Static::Rigid { value, .. } => {
            Some(display_name(value, door.types, door.symbols()).to_string())
        }
    }
}

/// The identity of the content tree the binder `name` of `SUBJECT` holds, as `facts` holds it.
fn identity<'a, S: RecordSource<'a>>(
    door: &Door<'_, '_>,
    facts: &FactsOver<S>,
    name: &str,
) -> Option<Identity> {
    let f = subject(door.root());
    (facts.content(f, slot(door.symbols(), f, name))).map(|tree| tree.identity())
}

/// The site of the node `LET name = <node>` binds in `shape`.
fn node_of(symbols: &SymbolInterner, shape: &BodyShape<'_>, name: &str) -> Site {
    let Some(ExpressionPart::Expression(node)) = shape.rhs(slot(symbols, shape, name)) else {
        panic!("`{name}` is bound to a node");
    };
    shape.site_of_node(node)
}

/// The body of the one quote the program writes.
fn code<'graph>(door: &Door<'_, 'graph>) -> &'graph BodyShape<'graph> {
    (door.load().shapes().bodies().iter())
        .find(|shape| shape.kind() == ShapeKind::Code)
        .expect("the program quotes code")
}

/// `value` with each level `levels` binds read as its type, each derived name its term handed
/// again with them written in, where every name it reads is bound: `None` where one is not.
fn substitute(
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
    let open =
        (crate::scope::free_variables(door.types, &scratch, read.into()).iter()).any(|variable| {
            matches!(
                variable,
                Variable::Lexical { .. } | Variable::Path { .. } | Variable::Derived { .. }
            )
        });
    (!open).then_some(read)
}

/// A binding of a module parameter to a module carried at a signature pinning its carrier selects
/// the overload a use over the parameter's member keeps two candidates of at the load.
#[test]
fn binding_a_module_parameter_closes_its_paths() {
    let source = format!(
        "{COUNTER}{PICK}{PINNED}\
         LET step = (FN :{{m :Counter}} -> Any = #(\n  LET p = (PICK m.zero)\n  p\n))\n"
    );
    let (before, after) = opened(&source, |door| {
        let symbols = door.symbols();
        let step = born(symbols, door.root(), "step");
        let before = narrowing(door, door.load(), step, "p");
        let pinned = framed(door, "pinned", "ints");
        let under = (door.under(step, &[(unknown(symbols, step, "m"), pinned)]))
            .expect("the binding refuses nothing");
        (before, narrowing(door, &under, step, "p"))
    });
    assert_eq!(before, "kept maybe maybe");
    assert_eq!(after, "selected");
}

/// A binding of a path's root module beside a `FOR ALL` name reads the path as a path, not as a
/// projection out of the type the root is bound to: it refuses nothing.
#[test]
fn binding_a_module_binder_beside_a_name_keeps_its_paths() {
    let source = format!(
        "{COUNTER}{PINNED}\
         EXPR FOR ALL #[Elt] #(WRAP x :Elt AND m :Counter) -> Any = #(\n  \
         LET s :{{a :Elt b :(m.Carrier)}} = {{a = x, b = m.zero}}\n  x\n)"
    );
    let under = opened(&source, |door| {
        let symbols = door.symbols();
        let f = quantified(door);
        let staged = [
            (unknown(symbols, f, "Elt"), Bound::Type(KType::NUMBER)),
            (unknown(symbols, f, "m"), framed(door, "pinned", "ints")),
        ];
        door.under(f, &staged).map(|_| ())
    });
    assert_eq!(under, Ok(()));
}

/// A binding of a module parameter is written into the path variables of its classes: the
/// parameter's entry record holds its seed's path, the seed under the binding is the type the
/// retype stamps, and a read of its member is what a fresh load reads.
#[test]
fn binding_a_module_parameter_writes_it_into_its_paths() {
    let source = format!(
        "{COUNTER}{STAMPED_COUNTER}\
         LET step = (FN :{{m :Counter}} -> Any = #(\n  LET k = m.zero\n  k\n))\n"
    );
    let (held, seed, read) = opened(&source, |door| {
        let symbols = door.symbols();
        let step = born(symbols, door.root(), "step");
        let (entry, _) = (door.load().unit(unit_of(door, step, "m"))).expect("the entry ran");
        let staged = [(unknown(symbols, step, "m"), framed(door, "stamped", "ints"))];
        let under = (door.under(step, &staged)).expect("the binding refuses nothing");
        (
            entry.paths.len(),
            upper(door, &under, step, "m"),
            upper(door, &under, step, "k"),
        )
    });
    assert_eq!(held, 1, "the entry's record holds the seed's path");
    assert!(seed.ends_with("WITH {Carrier = Number}"), "{seed}");
    assert_eq!(read, "Number");
}

/// A path read through a binder a `LET` retypes at a signature, over a module parameter, closes by
/// module substitution: under a binding of the parameter, `n.Carrier` is the carrier the retyped
/// module shows, as a fresh load reads it.
#[test]
fn a_path_through_a_retyped_binder_closes_as_the_module_it_shows() {
    let source = format!(
        "{COUNTER}{STAMPED_COUNTER}\
         LET step = (FN :{{m :Counter}} -> Any = #(\n  LET n :Counter = m\n  \
         LET Carriers = :(n.Carrier)\n  Carriers\n))\n"
    );
    let (load, under) = opened(&source, |door| {
        let symbols = door.symbols();
        let step = born(symbols, door.root(), "step");
        let load = declares_closed(door, door.load(), step, "Carriers");
        let staged = [(unknown(symbols, step, "m"), framed(door, "stamped", "ints"))];
        let under = (door.under(step, &staged)).expect("the binding refuses nothing");
        let at = slot(symbols, step, "Carriers");
        let under = match under.declared_type(door.types, step, at) {
            Static::Closed(closed) => Some(display_name(closed, door.types, symbols).to_string()),
            _ => None,
        };
        (load, under)
    });
    assert!(!load, "the load reads `n.Carrier` as its class");
    assert_eq!(under.as_deref(), Some("Number"));
}

/// A module parameter held in place is pinned at the paths through its operand, so a read of it
/// is over `m`'s own class at the load, and under a binding of `m` is what a fresh load reads. A
/// hold of a hold, a hold of a member chain, and a hold of a `:Module` parameter are pinned at the
/// same paths, and a member read off the hold reads the type it arrives at.
#[test]
fn a_module_held_in_place_is_pinned_at_its_operand_s_paths() {
    let prelude = format!(
        "{COUNTER}{STAMPED_COUNTER}\
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
        let (load, under) = opened(&program(param, ""), |door| {
            let symbols = door.symbols();
            let f = subject(door.root());
            let (elt, m) = (unknown(symbols, f, "Elt"), unknown(symbols, f, "m"));
            let staged = [
                (elt, Bound::Type(KType::NUMBER)),
                (
                    m,
                    framed(
                        door,
                        module,
                        if module == "stamped" { "ints" } else { module },
                    ),
                ),
            ];
            let under = (door.under(f, &staged)).expect("no refusal");
            (
                upper(door, door.load(), f, "s0"),
                upper(door, &under, f, "s0"),
            )
        });
        let restart = program("n :Any", &format!("LET m = {module}\n  "));
        let restarted = opened(&restart, |door| {
            upper(door, door.load(), subject(door.root()), "s0")
        });
        assert_ne!(
            load, "Any",
            "{body}: the load reads the hold over `m`'s class"
        );
        assert_eq!(under, restarted, "{body}");
        assert_eq!(under, "Number", "{body}");
    }
}

/// A value member read through a union over a type a run binds, whose bound declares it, is the
/// member the binding's module declares, read whole over the union: under a binding of `Elt`, the
/// load's type closed by the binding is the frame's, and a fresh load's. A read through a union
/// whose run-bound name's bound declares no such member is refused at load.
#[test]
fn a_member_read_through_a_run_bound_name_is_what_a_fresh_load_reads() {
    let prefix = "SIG Has = #[(VAL f :Number)]\nSIG Named = #[(VAL f :Str)]\n";
    let (body, declared, expected) = (
        "#(SUBJECT x :(Elt | Has)) -> Any = #(\n  LET y = x.f\n  y\n)",
        ":Never",
        "[Number, Number]",
    );
    let unbounded = format!("{prefix}EXPR FOR ALL #[Elt] {body}");
    assert_eq!(
        super::run(&unbounded),
        "load: <test>:4:11: :(Elt | SIG (f: Number)) has no field f: its bound Any declares none; a \
         body reads a field only some bindings have through `MATCH … WITH`"
    );
    let source = format!("{prefix}LET Bound = {declared}\nEXPR FOR ALL #{{Elt: Named}} {body}");
    let (closed, under) = opened(&source, |door| {
        let f = subject(door.root());
        let elt = unknown(door.symbols(), f, "Elt");
        let at = slot(door.symbols(), f, "y");
        let Some(crate::scope::Holding::Type(load)) = door.load().binder_type(door.types, f, at)
        else {
            panic!("`y` is typed, and no scheme");
        };
        let bound = door.declared("Bound");
        let levels: Vec<(usize, KType)> = (door.load().homes(f).iter())
            .filter_map(|(level, target)| match target {
                Target::Local(slot)
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
            let closed = substitute(door, &levels, ty).expect("the binding closes it");
            display_name(closed, door.types, door.symbols()).to_string()
        };
        let closed = format!("[{}, {}]", show(load.lower), show(load.upper));
        let under = (door.under(f, &[(elt, Bound::Type(bound))])).expect("no refusal");
        (closed, ends(door, &under, f, "y"))
    });
    assert_eq!(closed, under, "{body} under `Elt := {declared}`");
    assert_eq!(under, expected, "{body} under `Elt := {declared}`");
    let restart = format!("{prefix}LET Elt = {declared}\nEXPR {body}");
    let restarted = opened(&restart, |door| {
        ends(door, door.load(), subject(door.root()), "y")
    });
    assert_eq!(under, restarted, "{body} under `Elt := {declared}`");
}

/// A path through a module parameter whose declared application pins its label over a run-bound
/// name is that pin: under a binding of the name it is what a fresh load with the name declared
/// reads.
#[test]
fn a_path_pinned_over_a_run_bound_name_is_its_pin() {
    let prelude = "SIG Paired FOR ALL #[Left Right] = #[(VAL left :Left) (VAL right :Right)]\n";
    let body = "#(WRAP x :Elt AND p :(Paired WITH {Left = Elt})) -> Any = #(\n  \
                LET y :(p.Left) = p.left\n  LET Lefts = :(p.Left)\n  y\n)\n";
    let source = format!("{prelude}EXPR FOR ALL #[Elt] {body}");
    let under = opened(&source, |door| {
        let f = subject(door.root());
        let elt = unknown(door.symbols(), f, "Elt");
        let under = (door.under(f, &[(elt, Bound::Type(KType::NUMBER))])).expect("no refusal");
        ends(door, &under, f, "y")
    });
    let restarted = opened(&format!("{prelude}LET Elt = Number\nEXPR {body}"), |door| {
        ends(door, door.load(), subject(door.root()), "y")
    });
    assert_eq!(under, restarted);
    assert_eq!(under, "[Number, Number]");
}

/// A head a `USING` surfaces through an ascription pinned over a run-bound name registers at its
/// pins over the name: under a binding of the name the call is what a fresh load with the name
/// declared selects.
#[test]
fn a_head_surfaced_through_an_ascription_over_a_name_registers_over_it() {
    let body = "#(SUBJECT x :Elt AND p :Picks) -> Any = #(\n  \
                LET s0 = (USING (p :! (Picks WITH {Carrier = Elt})) SCOPE (PICK zero))\n  s0\n)\n";
    let source = format!("{PICKS}EXPR FOR ALL #[Elt] {body}");
    let (load, under) = opened(&source, |door| {
        let f = subject(door.root());
        let elt = unknown(door.symbols(), f, "Elt");
        let under = (door.under(f, &[(elt, Bound::Type(KType::NUMBER))])).expect("no refusal");
        (
            upper(door, door.load(), f, "s0"),
            ends(door, &under, f, "s0"),
        )
    });
    let restarted = opened(&format!("{PICKS}LET Elt = Number\nEXPR {body}"), |door| {
        ends(door, door.load(), subject(door.root()), "s0")
    });
    assert!(
        !load.contains("Any"),
        "the load registers the head over `Elt`: {load}"
    );
    assert_eq!(under, restarted);
    assert_eq!(under, "[Str, Str]");
}

/// A `MODULE` body whose members are written over a run-bound name is exactly the signature they
/// declare, written over the name: under a binding of the name it is what a fresh load with the
/// name declared types it.
#[test]
fn a_module_over_a_run_bound_name_is_its_signature_over_the_name() {
    let body = "#(SUBJECT x :Elt) -> Any = #(\n  \
                MODULE md0 OVER #[Elt] = (LET f = (FN :{y :Elt} -> Elt = #(y)))\n  md0\n)\n";
    let source = format!("EXPR FOR ALL #[Elt] {body}");
    let under = opened(&source, |door| {
        let f = subject(door.root());
        let elt = unknown(door.symbols(), f, "Elt");
        let under = (door.under(f, &[(elt, Bound::Type(KType::NUMBER))])).expect("no refusal");
        ends(door, &under, f, "md0")
    });
    let restarted = opened(&format!("LET Elt = Number\nEXPR {body}"), |door| {
        ends(door, door.load(), subject(door.root()), "md0")
    });
    assert_eq!(under, restarted);
}

/// A `MODULE` body capturing a run-bound name, or a local alias written over one, holds a content
/// tree a binding closes: open at the load, closed under a binding, and over an alias the tree a
/// fresh load with the name declared reads.
#[test]
#[ignore = "the door reads no content tree under a binding: `Facts::content` is the load's record"]
fn a_module_capturing_a_run_bound_name_closes_its_tree_under_a_binding() {
    let bodies = [
        "#(SUBJECT x :Elt) -> Any = #(\n  MODULE md0 OVER #[Elt] = (LET f = 7)\n  md0\n)\n",
        "#(SUBJECT x :Elt) -> Any = #(\n  LET Row = :(LIST OF Elt)\n  \
         MODULE md0 OVER #[Row] = (LET f = (FN :{y :Row} -> Row = #(y)))\n  md0\n)\n",
    ];
    for body in bodies {
        let (load, under) = opened(&format!("EXPR FOR ALL #[Elt] {body}"), |door| {
            let f = subject(door.root());
            let (elt, at) = (
                unknown(door.symbols(), f, "Elt"),
                slot(door.symbols(), f, "md0"),
            );
            let held = "`md0` holds a content tree";
            let load = door.load().content(f, at).expect(held);
            let under = (door.under(f, &[(elt, Bound::Type(KType::NUMBER))])).expect("no refusal");
            (load.closed(), under.content(f, at).expect(held).identity())
        });
        assert!(!load, "the load's tree is open over `Elt`:\n{body}");
        assert!(matches!(under, Identity::Closed(_)), "{under:?}\n{body}");
        if body.contains("Row") {
            let restarted = opened(&format!("LET Elt = Number\nEXPR {body}"), |door| {
                identity(door, door.load(), "md0").expect("`md0` holds a content tree")
            });
            assert_eq!(under, restarted, "{body}");
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
    let (held, read) = opened(&source, |door| {
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
        "{COUNTER}{STAMPED_COUNTER}\
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
            let admitted = opened(&source, |door| {
                let step = born(door.symbols(), door.root(), "step");
                let m = unknown(door.symbols(), step, "m");
                door.under(
                    step,
                    &[(
                        m,
                        framed(
                            door,
                            module,
                            if module == "stamped" { "ints" } else { "strs" },
                        ),
                    )],
                )
                .is_ok()
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
            assert_eq!(admitted, loaded, "{body} under `m := {module}`");
            assert_eq!(admitted, fits, "{body} under `m := {module}`");
        }
    }
}

/// A record holding a content tree with a leaf on a module parameter names the parameter: under a
/// binding of it, its tree reads as the bound module's, as a fresh load reads it.
#[test]
#[ignore = "the door reads no content tree under a binding: `Facts::content` is the load's record"]
fn binding_a_module_parameter_substitutes_it_into_the_trees_its_records_hold() {
    let source = format!(
        "{COUNTER}{STAMPED_COUNTER}\
         LET step = (FN :{{m :Counter}} -> Any = #(\n  LET n = m\n  n\n))\n"
    );
    let (before, after, bound) = opened(&source, |door| {
        let symbols = door.symbols();
        let step = born(symbols, door.root(), "step");
        let n = slot(symbols, step, "n");
        let before = door.load().content(step, n).map(|tree| tree.closed());
        let stamped = door.module("stamped");
        let bound = match stamped {
            Bound::Module { tree, .. } => tree.identity(),
            Bound::Type(_) | Bound::Carried(_) => unreachable!("`stamped` is a module"),
        };
        let staged = [(unknown(symbols, step, "m"), framed(door, "stamped", "ints"))];
        let under = (door.under(step, &staged)).expect("the binding refuses nothing");
        (
            before,
            under.content(step, n).map(|tree| tree.identity()),
            bound,
        )
    });
    assert_eq!(before, Some(false), "the load reads `n` as a leaf on `m`");
    assert_eq!(
        after,
        Some(bound),
        "a binding reads `n` as the module it binds"
    );
}

/// A type binder that reads a `FOR ALL` name before a path over a module parameter the binding
/// leaves unbound declares unknown at the load, and stays unknown under a binding of the name: its
/// path is a class.
#[test]
fn binding_a_name_a_type_binder_reads_before_an_unbound_path_leaves_it_unknown() {
    let source = format!(
        "{COUNTER}EXPR FOR ALL #[Elt] #(WRAP x :Elt AND m :Counter) -> Any = #(\n  \
         LET Both = :{{a :Elt b :(m.Carrier)}}\n  x\n)"
    );
    let (before, after) = opened(&source, |door| {
        let f = quantified(door);
        let before = declares_closed(door, door.load(), f, "Both");
        let elt = unknown(door.symbols(), f, "Elt");
        let under = (door.under(f, &[(elt, Bound::Type(KType::NUMBER))]))
            .expect("the binding refuses nothing");
        (before, declares_closed(door, &under, f, "Both"))
    });
    assert!(!before, "the load leaves the binder unknown");
    assert!(!after, "the binder stays unknown over the unbound path");
}

/// A type expression the load leaves unknown at a meet over a `FOR ALL` name, before a path: a
/// binding of the name finds the path's entry, and refuses nothing.
#[test]
fn binding_a_name_a_type_reads_before_a_path_finds_its_entry() {
    let source = format!(
        "{COUNTER}EXPR FOR ALL #[Elt] #(WRAP x :Elt AND m :Counter) -> Any = #(\n  \
         LET s :{{a :(Elt & Number) b :(m.Carrier)}} = {{a = x, b = m.zero}}\n  x\n)"
    );
    let under = opened(&source, |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        door.under(f, &[(elt, Bound::Type(KType::NUMBER))])
            .map(|_| ())
    });
    assert_eq!(under, Ok(()));
}

/// A quote's code read under a binding of its `$` capture decides what reads it: the load types
/// `$r.zero` over the type the module `$r` holds carries, which binding `$r` to `ints` in the
/// frame running the code closes.
#[test]
fn an_eval_frame_types_its_code_over_its_dollar_names_as_they_bind() {
    opened(USE, |door| {
        let code = code(door);
        let capture = (code.captures().iter())
            .position(|capture| door.symbols().display(capture.name.symbol()).to_string() == "r")
            .expect("the code captures `r`");
        let loaded =
            (door.load().statement_type(door.types, code, 0)).expect("the load types `$r.zero`");
        assert!(!loaded.is_exact(), "the load leaves `$r.zero` open");
        let dollar = Unknown {
            segment: code.segment(),
            at: Address::Capture(CaptureSlot(capture as u32)),
        };
        let under = (door.under(code, &[(dollar, framed(door, "ints", "ints"))]))
            .expect("the binding refuses nothing");
        let decided =
            (under.statement_type(door.types, code, 0)).expect("the frame types `$r.zero`");
        assert_eq!(decided, Interval::point(KType::NUMBER.into()));
    });
}

/// A quote's `$` capture of a module binder is bound as its source: under `r` bound to `ints`,
/// `USE`'s code types `$r.zero` as a binding of `$r` itself does.
#[test]
#[ignore = "the door binds no quote's `$` capture from its source binder: `$r.zero` stays over `r`"]
fn a_binding_of_a_captured_module_binder_closes_its_capture_as_its_source() {
    opened(USE, |door| {
        let code = code(door);
        let r = unknown(door.symbols(), subject(door.root()), "r");
        let under = (door.under(subject(door.root()), &[(r, framed(door, "ints", "ints"))]))
            .expect("the binding refuses nothing");
        assert_eq!(
            under.statement_type(door.types, code, 0),
            Some(Interval::point(KType::NUMBER.into()))
        );
    });
}

/// A module unknown binds to a closed content tree, so a view over a bound module parameter is
/// over a closed tree: the load sees an open one, and the frame reads the closed tree the binding
/// substitutes.
#[test]
#[ignore = "the door reads no view tree under a binding: `Facts::view_tree` is the load's record"]
fn a_view_over_a_bound_module_is_over_a_closed_tree() {
    let source = format!(
        "{COUNTER}{BOUND_COUNTER}\
         EXPR #(SUBJECT m :Counter) -> Any = #(\n  LET v = (m :| Counter)\n  v\n)\n"
    );
    let (load, under) = opened(&source, |door| {
        let body = subject(door.root());
        let site = node_of(door.symbols(), body, "v");
        let m = unknown(door.symbols(), body, "m");
        let under = (door.under(body, &[(m, framed(door, "stamped", "bound"))]))
            .expect("the binding refuses nothing");
        (
            (door.load().view_tree(body, site)).map(|tree| tree.closed()),
            under.view_tree(body, site).map(|tree| tree.closed()),
        )
    });
    assert_eq!(load, Some(false), "the load sees an open tree");
    assert_eq!(under, Some(true), "the bound tree is closed");
}

/// A view a binder holds over a module parameter is the view rule over the paths through its tree
/// at the load, and the view a fresh load builds under a binding of the parameter: its type closed
/// through the carrier the substituted view hides.
#[test]
fn a_view_a_binder_holds_is_the_view_rule_under_every_binding() {
    let body = "EXPR #(SUBJECT m :Counter) -> Any = #(\n  LET w = (m :| Counter)\n  w\n)\n";
    let source = format!("{COUNTER}{BOUND_COUNTER}{body}");
    let (load, under) = opened(&source, |door| {
        let f = subject(door.root());
        let m = unknown(door.symbols(), f, "m");
        let under = (door.under(f, &[(m, framed(door, "stamped", "bound"))]))
            .expect("the binding refuses nothing");
        (
            upper(door, door.load(), f, "w"),
            upper(door, &under, f, "w"),
        )
    });
    let restart = format!(
        "{COUNTER}{BOUND_COUNTER}EXPR #(SUBJECT n :Any) -> Any = #(\n  LET m = stamped\n  \
         LET w = (m :| Counter)\n  w\n)\n"
    );
    let restarted = opened(&restart, |door| {
        upper(door, door.load(), subject(door.root()), "w")
    });
    assert!(load.starts_with("(VIEW OF "), "{load}");
    assert_eq!(under, restarted);
}

/// A view written in place over a module parameter is the view rule over the paths through its
/// own site at the load, and the view a fresh load builds under a binding of the parameter: its
/// type closed through the carrier the substituted view hides.
#[test]
fn a_view_written_in_place_is_the_view_rule_under_every_binding() {
    let body = "#(\n  LET k = 1\n  (m :| Counter)\n)\n";
    fn viewed<'a, S: RecordSource<'a>>(door: &Door<'_, '_>, facts: &FactsOver<S>) -> String {
        let typed = (facts.statement_type(door.types, subject(door.root()), 1))
            .expect("the view's statement is typed");
        display_name(typed.upper, door.types, door.symbols()).to_string()
    }
    let source = format!("{COUNTER}{BOUND_COUNTER}EXPR #(SUBJECT m :Counter) -> Any = {body}");
    let (load, under) = opened(&source, |door| {
        let f = subject(door.root());
        let m = unknown(door.symbols(), f, "m");
        let under = (door.under(f, &[(m, framed(door, "stamped", "bound"))]))
            .expect("the binding refuses nothing");
        (viewed(door, door.load()), viewed(door, &under))
    });
    let restart = format!(
        "{COUNTER}{BOUND_COUNTER}EXPR #(SUBJECT n :Any) -> Any = \
         #(\n  LET m = stamped\n  (m :| Counter)\n)\n"
    );
    let restarted = opened(&restart, |door| viewed(door, door.load()));
    assert!(load.starts_with("(VIEW OF "), "{load}");
    assert_eq!(under, restarted);
}

/// A local reading a view over a module parameter reads the view's own paths, which a binding of
/// the parameter closes by tree substitution: under the parameter's binding it is typed as a fresh
/// load types it.
#[test]
fn a_local_reading_a_view_over_a_bound_module_is_what_a_fresh_load_reads() {
    let body = "LET w = (m :| Counter)\n  LET k = w.zero\n  k\n)\n";
    let source =
        format!("{COUNTER}{BOUND_COUNTER}EXPR #(SUBJECT m :Counter) -> Any = #(\n  {body}");
    let under = opened(&source, |door| {
        let f = subject(door.root());
        let m = unknown(door.symbols(), f, "m");
        let under = (door.under(f, &[(m, framed(door, "stamped", "bound"))]))
            .expect("the binding refuses nothing");
        upper(door, &under, f, "k")
    });
    let restart = format!(
        "{COUNTER}{BOUND_COUNTER}EXPR #(SUBJECT n :Any) -> Any = #(\n  LET m = stamped\n  {body}"
    );
    let restarted = opened(&restart, |door| {
        upper(door, door.load(), subject(door.root()), "k")
    });
    assert_eq!(under, restarted);
}

/// A `:Module` parameter is exactly the type its argument carries, the derived name a binding of
/// it gives: a reader of it, of a list over it, of a view of it or of its member is typed under a
/// binding of it as a fresh load types it.
#[test]
fn a_module_parameter_is_the_type_its_argument_carries() {
    let module = "MODULE bound = ((LET Carrier = Number) (LET zero = 0) \
                  (LET succ = (FN :{x :Number} -> Number = #(x))))\n\
                  LET stamped :Module = bound\n";
    for body in [
        "LET k = m\n  k\n)\n",
        "LET k = [m]\n  k\n)\n",
        "LET k = (m :| Counter)\n  k\n)\n",
        "LET k = m.zero\n  k\n)\n",
    ] {
        let source = format!("{COUNTER}{module}EXPR #(SUBJECT m :Module) -> Any = #(\n  {body}");
        let under = opened(&source, |door| {
            let f = subject(door.root());
            let m = unknown(door.symbols(), f, "m");
            let under = (door.under(f, &[(m, framed(door, "stamped", "bound"))]))
                .expect("the binding refuses nothing");
            ends(door, &under, f, "k")
        });
        let restart = format!(
            "{COUNTER}{module}EXPR #(SUBJECT n :Any) -> Any = #(\n  LET m = stamped\n  {body}"
        );
        let restarted = opened(&restart, |door| {
            ends(door, door.load(), subject(door.root()), "k")
        });
        assert_eq!(under, restarted, "{body}");
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
        let (loaded, under) = opened(&body("FOR ALL #[Elt] #(SUBJECT x :Elt)", &inner), |door| {
            let f = subject(door.root());
            let elt = unknown(door.symbols(), f, "Elt");
            let under = (door.under(f, &[(elt, Bound::Type(KType::NUMBER))]))
                .expect("the binding refuses nothing");
            (ends(door, door.load(), f, "z"), ends(door, &under, f, "z"))
        });
        let declared = inner.replace(":Elt", ":Number");
        let restarted = opened(&body("#(SUBJECT x :Number)", &declared), |door| {
            ends(door, door.load(), subject(door.root()), "z")
        });
        assert_eq!(loaded, "[Number, Number]", "{outer}{inner}");
        assert_eq!(under, restarted, "{outer}{inner}");
    }
}

/// A closure over `Elt` called by name unfolds, and a module viewed at an application over `Elt`
/// is a view at it: each binder's tree under `Elt := Number` is a fresh load's with `Number`
/// written for `Elt`, and the call's member reads `Number` at load already.
#[test]
#[ignore = "the door reads no content tree under a binding: `Facts::content` is the load's record"]
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
    fn trees<'a, S: RecordSource<'a>>(
        door: &Door<'_, '_>,
        facts: &FactsOver<S>,
    ) -> (Option<Identity>, Option<Identity>, String) {
        let f = subject(door.root());
        (
            identity(door, facts, "a"),
            identity(door, facts, "v"),
            ends(door, facts, f, "z"),
        )
    }
    let (loaded, under) = opened(&body("FOR ALL #[Elt] #(SUBJECT x :Elt)", "Elt"), |door| {
        let elt = unknown(door.symbols(), subject(door.root()), "Elt");
        let under = (door.under(subject(door.root()), &[(elt, Bound::Type(KType::NUMBER))]))
            .expect("refuses nothing");
        (trees(door, door.load()), trees(door, &under))
    });
    let fresh = opened(&body("#(SUBJECT x :Number)", "Number"), |door| {
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
#[ignore = "the door reads no content tree under a binding: `Facts::content` is the load's record"]
fn a_type_literal_over_a_name_is_the_type_value_a_binding_closes() {
    let body = |declared: &str, elt: &str| {
        format!(
            "EXPR {declared} -> Any = #(\n  LET t = :(LIST OF {elt})\n  \
             MODULE built OVER #[t] = (LET Zero = Number)\n  LET a = built\n  a\n)\n"
        )
    };
    let under = opened(&body("FOR ALL #[Elt] #(SUBJECT x :Elt)", "Elt"), |door| {
        let elt = unknown(door.symbols(), subject(door.root()), "Elt");
        let under = (door.under(subject(door.root()), &[(elt, Bound::Type(KType::NUMBER))]))
            .expect("refuses nothing");
        identity(door, &under, "a")
    });
    let fresh = opened(&body("#(SUBJECT x :Number)", "Number"), |door| {
        identity(door, door.load(), "a")
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
#[ignore = "the door reads no content tree under a binding: `Facts::content` is the load's record"]
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
    fn pair<'a, S: RecordSource<'a>>(
        door: &Door<'_, '_>,
        facts: &FactsOver<S>,
    ) -> (Option<Identity>, Option<Identity>) {
        (identity(door, facts, "a"), identity(door, facts, "b"))
    }
    let (load, either, number, z) = opened(&body("FOR ALL #[Elt] #(SUBJECT x :Elt)"), |door| {
        let f = subject(door.root());
        let elt = unknown(door.symbols(), f, "Elt");
        let either = Bound::Type(door.declared("Either"));
        let either = (door.under(f, &[(elt, either)])).expect("refuses nothing");
        let number =
            (door.under(f, &[(elt, Bound::Type(KType::NUMBER))])).expect("refuses nothing");
        (
            pair(door, door.load()),
            pair(door, &either),
            pair(door, &number),
            ends(door, &number, f, "z"),
        )
    });
    let (fresh, fresh_z) = opened(&body("#(SUBJECT x :Number)"), |door| {
        let f = subject(door.root());
        (pair(door, door.load()), ends(door, door.load(), f, "z"))
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

/// `p.Right` reads through `Paired WITH {Left = Elt}`: binding `Elt` alone moves its paths to the
/// class over `Paired WITH {Left = Number}`, which a fresh load with `Elt` declared names; binding
/// `p` beside it closes them to the module's `Right`, as a fresh load with both declared reads it.
#[test]
fn binding_a_path_s_application_alone_moves_it_to_the_class_a_fresh_load_names() {
    let body = "#(WRAP x :Elt AND p :(Paired WITH {Left = Elt})) -> Any = #(\n  \
                LET k = p.right\n  LET j :(p.Right) = k\n  j\n)\n";
    let source = format!("{PAIRED}EXPR FOR ALL #[Elt] {body}");
    let (alone, closed) = opened(&source, |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let p = unknown(door.symbols(), f, "p");
        let alone = (door.under(f, &[(elt, Bound::Type(KType::NUMBER))])).expect("refuses nothing");
        let both = [
            (elt, Bound::Type(KType::NUMBER)),
            (p, framed(door, "stamped", "pair")),
        ];
        let closed = (door.under(f, &both)).expect("refuses nothing");
        (upper(door, &alone, f, "k"), upper(door, &closed, f, "k"))
    });
    let named = opened(&format!("{PAIRED}LET Elt = Number\nEXPR {body}"), |door| {
        upper(door, door.load(), subject(door.root()), "k")
    });
    let restart = format!(
        "{PAIRED}LET Elt = Number\nEXPR #(WRAP x :Elt) -> Any = #(\n  LET p = stamped\n  \
         LET k = p.right\n  LET j :(p.Right) = k\n  j\n)\n"
    );
    let restarted = opened(&restart, |door| {
        upper(door, door.load(), subject(door.root()), "k")
    });
    assert_eq!(alone, named);
    assert_eq!(closed, restarted);
}

/// A `USING` header over a closed module body, retyped to an application over a run-bound name,
/// picks the body's registrations by questions: whether each covers the header's keyworded member.
/// Under a binding it is what a fresh load with the name declared reads.
#[test]
fn header_picks_over_a_run_bound_name_are_asked() {
    let body = "#(SUBJECT x :Elt) -> Any = #(\n  \
                LET s0 = (USING (pk :! (Picks WITH {Carrier = Elt})) SCOPE (PICK zero))\n  s0\n)\n";
    let module = "MODULE pk = ((LET zero = 1) (EXPR #(PICK x :Number) -> Str = #(\"n\")))\n";
    let source = format!("{PICKS}{module}EXPR FOR ALL #[Elt] {body}");
    let under = opened(&source, |door| {
        let f = subject(door.root());
        let elt = unknown(door.symbols(), f, "Elt");
        let under = (door.under(f, &[(elt, Bound::Type(KType::NUMBER))])).expect("no refusal");
        ends(door, &under, f, "s0")
    });
    let restarted = opened(
        &format!("{PICKS}{module}LET Elt = Number\nEXPR {body}"),
        |door| ends(door, door.load(), subject(door.root()), "s0"),
    );
    assert_eq!(under, restarted);
}

/// A module binder the run builds whose static type is at most `Module` is typed by the name of
/// the type it carries, so a `:|` over it asks a question a binding answers: a binding of `a` to
/// `made` refuses the view as a fresh load with `a` declared as `made` refuses it.
#[test]
fn a_view_over_a_module_the_run_builds_is_refused_as_a_fresh_load_refuses_it() {
    let body =
        |a: &str| format!("#(SUBJECT x :Elt) -> Any = #(\n  LET a = {a}\n  a :| Counter\n)\n");
    let refused = opened(
        &format!("{MKU}EXPR FOR ALL #[Elt] {}", body("(MKU x)")),
        |door| {
            let f = quantified(door);
            let a = unknown(door.symbols(), f, "a");
            door.under(f, &[(a, framed(door, "made", "made"))]).err()
        },
    );
    let refused = refused.expect("the binding refuses the view");
    let restarted = super::run(&format!("{MKU}LET Elt = :Number\nEXPR {}", body("made")));
    let restarted = restarted
        .strip_prefix("load: ")
        .expect("the fresh load refuses");
    assert_eq!(unlocated(&refused), unlocated(restarted));
}

/// Two `LET` binders holding one call hold one tree: `z`, annotated through `a` and bound through
/// `b`, is typed by the content a frame binds both to, whichever statement comes first and
/// whichever holder the binding names first.
#[test]
fn two_holders_of_one_call_type_a_read_across_them_by_their_binding() {
    for (first, second) in [("a", "b"), ("b", "a")] {
        let source = format!(
            "{MKU}EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
             LET {first} = (MKU x)\n  LET {second} = (MKU x)\n  LET z :(a.Zero) = b.zero\n  z\n)\n"
        );
        let typed = opened(&source, |door| {
            let f = quantified(door);
            let x = (unknown(door.symbols(), f, "x"), Bound::Type(KType::NUMBER));
            ["a", "b"].map(|holder| {
                let made = |name| {
                    (
                        unknown(door.symbols(), f, name),
                        framed(door, "made", "made"),
                    )
                };
                let staged = [x, made(holder), made(if holder == "a" { "b" } else { "a" })];
                let under = (door.under(f, &staged)).expect("no refusal");
                ends(door, &under, f, "z")
            })
        });
        assert_eq!(typed[0], typed[1], "{first} first");
        assert_eq!(typed[0], "[Number, Number]", "{first} first");
    }
}

/// The type binder `Typed` of a `:Module` parameter's body, read three ways.
struct ModuleRead {
    /// `Typed` at load.
    at_load: Option<String>,
    /// `Typed` under a binding of `m` to `stamped`.
    under: Option<String>,
    /// `Typed` as a fresh load with `m` declared as `stamped` gives it.
    restarted: Option<String>,
}

/// [`ModuleRead`] of `Typed`, declared as `declared` in `SUBJECT`'s body over `m :Module`.
fn over_a_module_parameter(declared: &str) -> ModuleRead {
    let body = format!("#(\n  LET Typed = {declared}\n  Typed\n)\n");
    let source = format!("{STAMPED}EXPR #(SUBJECT m :Module) -> Any = {body}");
    let (at_load, under) = opened(&source, |door| {
        let f = subject(door.root());
        let m = unknown(door.symbols(), f, "m");
        let at_load = declared_as(door, door.load(), f, "Typed");
        let under = (door.under(f, &[(m, framed(door, "stamped", "bound"))]))
            .expect("the binding refuses nothing");
        (at_load, declared_as(door, &under, f, "Typed"))
    });
    let restart = format!(
        "{STAMPED}EXPR #(SUBJECT n :Any) -> Any = #(\n  LET m = stamped\n  LET Typed = {declared}\n  Typed\n)\n"
    );
    let restarted = opened(&restart, |door| {
        declared_as(door, door.load(), subject(door.root()), "Typed")
    });
    ModuleRead {
        at_load,
        under,
        restarted,
    }
}

/// A meet with a path's variable is the meet's derived name, which module substitution closes:
/// the load types it, and under `m := stamped` it is what a fresh load with `m` declared gives it.
#[test]
fn a_meet_with_a_paths_variable_is_a_derived_name_a_binding_closes() {
    let ModuleRead {
        at_load,
        under,
        restarted,
    } = over_a_module_parameter(":(Number & m.Carrier)");
    let at_load = at_load.expect("the load types the meet");
    assert!(at_load.starts_with('('), "a derived name: {at_load}");
    assert_eq!(under.as_deref(), Some("Number"));
    assert_eq!(under, restarted);
}

/// A type member a `MODULE` body binds closed is the module's own, though a value member stays
/// open: under `c := held`, with `Elt` left open, `t :(inner.Tag)` is what a fresh load with `c`
/// declared reads it, the carrier `c` binds, where the load leaves it over the path. A frame reads
/// `inner`'s content off its slot value: `activation`'s
/// `a_type_member_of_a_body_s_module_is_checked_at_run_as_a_fresh_load_checks_it` runs it.
#[test]
#[ignore = "the door reads no content tree under a binding: `inner`'s is its slot value's"]
fn a_type_member_a_binding_closes_is_read_off_a_body_with_a_member_left_open() {
    let source = format!(
        "{WRAPS}EXPR FOR ALL #[Elt] #(WRAP c :Counter extra :Elt) -> Any = #(\n  \
         MODULE inner OVER #[c extra] = ((LET Tag = c.Carrier) (LET base = c.zero) (LET more = extra))\n  \
         LET t :(inner.Tag) = 1\n  t\n)"
    );
    let (load, under) = opened(&source, |door| {
        let f = subject(door.root());
        let c = unknown(door.symbols(), f, "c");
        let extra = unknown(door.symbols(), f, "extra");
        let staged = [
            (c, framed(door, "held", "ints")),
            (extra, Bound::Type(KType::NUMBER)),
        ];
        let under = (door.under(f, &staged)).expect("the binding refuses nothing");
        (ends(door, door.load(), f, "t"), ends(door, &under, f, "t"))
    });
    let restart = format!(
        "{WRAPS}LET c = held\nEXPR FOR ALL #[Elt] #(WRAP extra :Elt) -> Any = #(\n  \
         MODULE inner OVER #[extra] = ((LET Tag = c.Carrier) (LET base = c.zero) (LET more = extra))\n  \
         LET t :(inner.Tag) = 1\n  t\n)"
    );
    let restarted = opened(&restart, |door| {
        ends(door, door.load(), subject(door.root()), "t")
    });
    assert_eq!(
        load, "[(RETYPED TO Tag), Tag]",
        "the load leaves `t` over `inner.Tag`"
    );
    assert_eq!(under, restarted);
}

/// Two binders holding one call are read as a fresh load reads them declared: under `ha` and `hb`
/// bound to a module carrying `Elt`'s type, as `MKUA x` builds, `z0` reads `hb.zero` and `v0` views
/// `ha` as a fresh load with both declared at that module does.
///
/// ```text
/// LET ha = (MKUA x)
/// LET hb = (MKUA x)
/// LET z0 :(ha.Zero) = hb.zero
/// LET v0 = (ha :| Zeroed)
/// ```
#[test]
fn two_binders_holding_one_call_are_read_as_a_fresh_load_reads_them() {
    let prelude = "SIG Zeroed = #[(VAL zero :Any)]\n\
                   MODULE boundNumber = ((LET Zero = Number) (LET zero = 1))\n\
                   LET stampedNumber :Module = boundNumber\n\
                   MODULE boundStr = ((LET Zero = Str) (LET zero = \"s\"))\n\
                   LET stampedStr :Module = boundStr\n";
    let built = |name: &str, typed: &str| {
        format!(
            "EXPR #(MKUA {name} :{typed}) -> Module = #(\n    \
             (MODULE built OVER #[{name}] = ((LET Zero = {typed}) (LET zero = {name})))\n    \
             built\n  )\n  "
        )
    };
    let body = |holders: &str| {
        format!(
            "#(SUBJECT x :Elt) -> Any = #(\n  {}{}{holders}\n  \
             LET z0 :(ha.Zero) = hb.zero\n  LET v0 = (ha :| Zeroed)\n  x\n)\n",
            built("n", "Number"),
            built("s", "Str"),
        )
    };
    let source = format!(
        "{prelude}EXPR FOR ALL #[Elt] {}",
        body("LET ha = (MKUA x)\n  LET hb = (MKUA x)")
    );
    for elt in ["Number", "Str"] {
        let under = opened(&format!("LET BoundElt = :{elt}\n{source}"), |door| {
            let f = quantified(door);
            let staged = [
                (
                    unknown(door.symbols(), f, "Elt"),
                    Bound::Type(door.declared("BoundElt")),
                ),
                (
                    unknown(door.symbols(), f, "x"),
                    Bound::Type(door.declared("BoundElt")),
                ),
                (
                    unknown(door.symbols(), f, "ha"),
                    framed(door, &format!("stamped{elt}"), &format!("bound{elt}")),
                ),
                (
                    unknown(door.symbols(), f, "hb"),
                    framed(door, &format!("stamped{elt}"), &format!("bound{elt}")),
                ),
            ];
            let under = (door.under(f, &staged)).map_err(|refusal| unlocated(&refusal))?;
            Ok::<_, String>(["z0", "v0"].map(|name| ends(door, &under, f, name)))
        });
        let restart = format!(
            "{prelude}LET Elt = :{elt}\nEXPR {}",
            body(&format!("LET ha = stamped{elt}\n  LET hb = stamped{elt}"))
        );
        let restarted = opened(&restart, |door| {
            let f = subject(door.root());
            Ok(["z0", "v0"].map(|name| ends(door, door.load(), f, name)))
        });
        assert_eq!(under, restarted, "under `Elt := {elt}`");
    }
}
