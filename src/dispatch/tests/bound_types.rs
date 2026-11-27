//! A frame's facts under a binding of its names: the load's records answered by the instances its
//! bind sites make, read through the test door ([`Door::under`]) and set beside what a fresh load
//! with the binding written as a declaration reads.

use crate::parse::ExpressionPart;
use crate::scope::{
    BodyShape, Bound, Elimination, Facts, FactsOver, Narrowing, RecordSource, ShapeKind, Static,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, display_name};

use super::under::laws::unlocated;
use super::under::{
    Door, ends, narrowing, opened, quantified, restarted_by, slot, subject, unit_of, unknown,
    waits_on,
};

/// Two overloads of `PICK`, one at `Number` and one at `Str`.
const PICK: &str = "EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                    EXPR #(PICK x :Str) -> Number = #(1)\n";

/// The body the binder `name` of `shape` births.
fn born<'graph>(
    door: &Door<'_, 'graph>,
    shape: &'graph BodyShape<'graph>,
    name: &str,
) -> &'graph BodyShape<'graph> {
    (shape.births(slot(door.symbols(), shape, name))).expect("the binder births a body")
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
    let (types, symbols) = (door.types, door.symbols());
    match facts.declared_type(types, shape, at) {
        Static::Unknown => None,
        Static::Closed(ty) => Some(display_name(ty, types, symbols).to_string()),
        Static::Rigid { value, .. } => Some(display_name(value, types, symbols).to_string()),
    }
}

/// How the use `LET name = <use>` in `shape` ranks its candidates, as `facts` holds it: open, by a
/// recorded table, or selected.
fn elimination<'a, S: RecordSource<'a>>(
    door: &Door<'_, '_>,
    facts: &FactsOver<S>,
    shape: &BodyShape<'_>,
    name: &str,
) -> &'static str {
    let Some(ExpressionPart::Expression(node)) = shape.rhs(slot(door.symbols(), shape, name))
    else {
        panic!("`{name}` is bound to a node");
    };
    let site = shape.site_of_node(node);
    match (facts.use_statics(door.types, door.bump, shape, site)).narrowing {
        Narrowing::Kept(kept) => match kept.elimination {
            Elimination::Open => "open",
            Elimination::Table(_) => "table",
        },
        Narrowing::Full => "open",
        Narrowing::Selected(_) => "selected",
    }
}

/// How many sites of `shape`'s statement `index` hold an ascription proven to fit, as `facts`
/// holds them.
fn proven_in<'a, S: RecordSource<'a>>(
    facts: &FactsOver<S>,
    shape: &BodyShape<'_>,
    index: usize,
) -> usize {
    let mut sites = Vec::new();
    crate::scope::each_part(&shape.body()[index], |part| sites.push(shape.site(part)));
    sites.push(shape.site_of_node(&shape.body()[index]));
    (sites.into_iter())
        .filter(|site| facts.settled(shape, *site))
        .count()
}

/// The ends of each binder of `names` in `SUBJECT`'s body as a fresh load of `restart` reads them
/// ([`restarted_by`]).
fn restarted<const N: usize>(restart: &str, names: [&str; N]) -> [String; N] {
    restarted_by(restart, |door| {
        let f = subject(door.root());
        names.map(|name| ends(door, door.load(), f, name))
    })
}

/// A rigid pair the load leaves unranked is ranked under a binding that closes it: `PICK y :Elt`
/// beside `PICK y :Str` could only be ranked by reading `Elt` through its bound, so the load
/// leaves the pair open, and under `Elt := Number` both are closed and the use ranks them by a
/// table, as a fresh load with `Elt` declared ranks them.
#[test]
fn a_rigid_pair_left_open_at_the_load_is_ranked_under_a_binding() {
    // `z.v`, a field typed `Any`, is no name a binding closes: the call keeps both candidates.
    let body = "#(WRAP x :Elt z :{v :Any}) -> Any = #(\n  \
                EXPR #(PICK y :Elt) -> Str = #(\"e\")\n  \
                EXPR #(PICK y :Str) -> Number = #(1)\n  \
                LET p = (PICK z.v)\n  p\n)";
    let (load, under) = opened(&format!("EXPR FOR ALL #[Elt] {body}"), |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let under = (door.under(f, &[(elt, Bound::Type(KType::NUMBER))])).expect("no refusal");
        (
            elimination(door, door.load(), f, "p"),
            elimination(door, &under, f, "p"),
        )
    });
    let restarted = opened(&format!("LET Elt = :Number\nEXPR {body}"), |door| {
        elimination(door, door.load(), subject(door.root()), "p")
    });
    assert_eq!(load, "open");
    assert_eq!(restarted, "table");
    assert_eq!(under, restarted);
}

/// A frame ranks a rigid pair by the table its binding closes: under `Elt := Number`, `PICK 5`
/// takes `PICK y :Elt` over `PICK y :Any`, and a string takes `PICK y :Str`; under `Elt := Bool`,
/// `PICK true` takes `PICK y :Elt`.
#[test]
fn a_frame_ranks_a_rigid_pair_by_the_table_its_binding_closes() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt z :Any) -> Any = #(\n  \
                  EXPR #(PICK y :Elt) -> Str = #(\"e\")\n  \
                  EXPR #(PICK y :Str) -> Str = #(\"s\")\n  \
                  EXPR #(PICK y :Any) -> Str = #(\"a\")\n  \
                  LET p = (PICK z)\n  p\n)\n\
                  PRINT (WRAP 1 5)\nPRINT (WRAP 1 \"t\")\nPRINT (WRAP 1 true)\n\
                  PRINT (WRAP true true)\n";
    assert_eq!(super::run(source), "e\ns\na\ne");
}

/// `OP` at `Number` and at `Str`, and `SUBJECT`'s body after `declared`, which runs a quote's code
/// calling `OP` over its `x :Elt`.
fn quoting(declared: &str) -> String {
    format!(
        "EXPR #(OP x :Number) -> Str = #(\"n\")\nEXPR #(OP x :Str) -> Number = #(1)\n\
         {declared}#(SUBJECT x :Elt) -> Any = #(\n  LET s0 = (EVAL #($(OP $x)) -> Any)\n  x\n)"
    )
}

/// The quote's code `SUBJECT`'s body holds.
fn code_of<'graph>(shape: &'graph BodyShape<'graph>) -> &'graph BodyShape<'graph> {
    let (_, code) = (shape.nested_shapes().iter())
        .find(|(_, nested)| nested.kind() == ShapeKind::Code)
        .expect("SUBJECT quotes a code");
    code
}

/// The refusal a fresh load with `Elt := Bool` declared holds for [`quoting`]'s code, worded, and
/// whether it types the code's statement.
fn quote_refused_fresh() -> (bool, String) {
    opened(&quoting("LET Elt = :Bool\nEXPR "), |door| {
        let code = code_of(subject(door.root()));
        let refusal = (door.load())
            .typing_refusal(door.types, door.bump, code, &mut |_| None)
            .map(|refusal| refusal.display(door.symbols(), door.types).to_string());
        (
            door.load().statement_type(door.types, code, 0).is_some(),
            refusal.expect("the fresh load refuses the code"),
        )
    })
}

/// A binding under which a quote's code refuses leaves nothing of the code to read: no `OP` takes
/// a `Bool`, so under `Elt := Bool` the code's frame refuses as a fresh load with `Elt` declared
/// refuses the code, which types nothing for it.
#[test]
fn a_binding_refusing_a_quote_s_code_hides_the_load_s_records_of_it() {
    let (loaded, under) = opened(&quoting("EXPR FOR ALL #[Elt] "), |door| {
        let f = quantified(door);
        let code = code_of(f);
        let elt = unknown(door.symbols(), f, "Elt");
        (
            door.load().statement_type(door.types, code, 0).is_some(),
            (door.under(code, &[(elt, Bound::Type(KType::BOOL))])).map(|_| ()),
        )
    });
    let (typed, refusal) = quote_refused_fresh();
    assert!(loaded, "the load types the code's statement");
    assert!(!typed, "a fresh load types nothing for it");
    let refused = under.expect_err("under `Bool` the code refuses");
    assert_eq!(unlocated(&refused), unlocated(&refusal));
}

/// A quote's code that refuses under a binding is refused where an `EVAL` runs it, never by the
/// frame holding it: under `Elt := Bool` `SUBJECT`'s frame refuses nothing, as a fresh load with
/// `Elt` declared loads `SUBJECT` and reports the code's refusal only from the `EVAL`.
#[test]
fn a_binding_refusing_a_quote_s_code_leaves_the_frame_holding_it_unrefused() {
    let own = opened(&quoting("EXPR FOR ALL #[Elt] "), |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        (door.under(f, &[(elt, Bound::Type(KType::BOOL))])).map(|_| ())
    });
    assert_eq!(own, Ok(()), "`SUBJECT`'s frame refuses nothing");
}

/// `WRAP`'s body: `y` holds `x :Elt` to `Number`, which is `Never` where `Elt` is, and `z` reads
/// `y` alone.
const ARRIVES: &str = "#(WRAP x :Elt) -> Any = #(\n  LET y :Number = x\n  LET z = y\n  z\n)";

/// A value held to a type is `Never` where it never arrives: at the load, the decided name over
/// whether `Elt` is `Never`, so a binding of `Elt` to `Never` gives `y` and `z`, which only reads
/// `y`, the types a fresh load with `Elt` declared gives them, and one to `Number` gives `Number`.
#[test]
fn a_value_that_never_arrives_is_never_under_the_binding_that_makes_it_so() {
    for (bound, declared) in [(KType::NEVER, ":Never"), (KType::NUMBER, ":Number")] {
        let (load, under) = opened(&format!("EXPR FOR ALL #[Elt] {ARRIVES}"), |door| {
            let f = quantified(door);
            let elt = unknown(door.symbols(), f, "Elt");
            let under = (door.under(f, &[(elt, Bound::Type(bound))])).expect("no refusal");
            (
                ["y", "z"].map(|name| upper(door, door.load(), f, name)),
                ["y", "z"].map(|name| upper(door, &under, f, name)),
            )
        });
        let restarted = opened(&format!("LET Elt = {declared}\nEXPR {ARRIVES}"), |door| {
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

/// The ends of each binder of `names` in `SUBJECT`'s body, a `FOR ALL` over `name` in `source`:
/// at the load, and under a binding of `name` to the type `source`'s top-level `Bound` declares.
fn load_and_under<const N: usize>(
    source: &str,
    name: &str,
    names: [&str; N],
) -> ([String; N], Result<[String; N], String>) {
    opened(source, |door| {
        let f = subject(door.root());
        let bound = Bound::Type(door.declared("Bound"));
        let under = door.under(f, &[(unknown(door.symbols(), f, name), bound)]);
        (
            names.map(|at| ends(door, door.load(), f, at)),
            under.map(|facts: Facts<'_>| names.map(|at| ends(door, &facts, f, at))),
        )
    })
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
        let (load, under) = load_and_under(&source, "Item", ["m", "t"]);
        let restarted = restarted(
            &format!("LET Item = {declared}\nEXPR {MATCHES}"),
            ["m", "t"],
        );
        assert_ne!(load, restarted, "the load leaves the arm to the binding");
        assert_eq!(under, Ok(restarted), "under `Item := {declared}`");
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
    let (load, under) = load_and_under(&source, "Outer", ["s", "t"]);
    let restarted = restarted(
        &format!("{APPLIES}LET Outer = {declared}\nEXPR {STORED}"),
        ["s", "t"],
    );
    assert_ne!(load, restarted, "the load leaves `s` to the binding");
    assert_eq!(under, Ok(restarted));
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
    let (load, under) = load_and_under(&source, "Item", ["s", "t"]);
    let restarted = restarted(
        &format!("{OPS}LET Item = {declared}\nEXPR {JUDGED}"),
        ["s", "t"],
    );
    assert!(
        load[0].contains("JUDGED"),
        "the load reads the judged names: {load:?}"
    );
    assert_eq!(under, Ok(restarted));
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
    let (load, under) = load_and_under(&source, "Outer", ["s", "t"]);
    let restarted = restarted(
        &format!("{RET}LET Outer = {declared}\nEXPR {CONSTRUCTS}"),
        ["s", "t"],
    );
    assert!(
        load[0].contains("CONSTRUCTED"),
        "the load reads the construction's name: {load:?}"
    );
    assert_eq!(under, Ok(restarted));
}

/// `Distance`, a newtype over `Number`.
const DISTANCES: &str = "NEWTYPE Distance = Number\n";

/// `SUBJECT`'s body: `s` constructs the newtype `Distance` over `y :Item`, and `t` reads `s`
/// alone.
const FITS: &str = "#(SUBJECT y :Item) -> Any = #(\n  LET s = (Distance y)\n  LET t = s\n  t\n)";

/// Whether `y :Item` fits `Distance`'s representation is a binding's to decide, so the
/// construction's lower end is the decided name on that fit, `Never` or the head: a binding of
/// `Item` gives `s` and `t` what a fresh load with `Item` declared gives them, under a fitting
/// payload and a misfit alike.
#[test]
fn a_newtype_construction_over_a_name_is_the_head_on_its_fit() {
    for declared in [":Number", ":Str"] {
        let source = format!("LET Bound = {declared}\n{DISTANCES}EXPR FOR ALL #[Item] {FITS}");
        let (load, under) = load_and_under(&source, "Item", ["s", "t"]);
        let restarted = restarted(
            &format!("{DISTANCES}LET Item = {declared}\nEXPR {FITS}"),
            ["s", "t"],
        );
        assert!(
            load[0].starts_with("[(ONE OF Never, Distance),"),
            "the load names the fit, never a misfit: {load:?}"
        );
        assert_eq!(under, Ok(restarted), "under `Item := {declared}`");
    }
}

/// `SUBJECT`'s body: `s` reads the list `xs` at `y :Item`, and `t` reads `s` alone.
const INDEXES: &str =
    "#(SUBJECT y :Item) -> Any = #(\n  LET xs = [10 20 30]\n  LET s = (xs y)\n  LET t = s\n  t\n)";

/// The ends of `s` and `t` in [`INDEXES`] at the load, under `Item := declared`, and as a fresh
/// load with `Item` declared reads them.
fn indexed(declared: &str) -> ([String; 2], Result<[String; 2], String>, [String; 2]) {
    let source = format!("LET Bound = {declared}\nEXPR FOR ALL #[Item] {INDEXES}");
    let (load, under) = load_and_under(&source, "Item", ["s", "t"]);
    let restarted = restarted(
        &format!("LET Item = {declared}\nEXPR {INDEXES}"),
        ["s", "t"],
    );
    (load, under, restarted)
}

/// Whether `y :Item` is a number is a binding's to decide, so the read's `Ok` is the decided name
/// on it: a binding of `Item` gives `s` and `t` what a fresh load with `Item` declared gives them,
/// the element alone under `Number` and the wide read under `Any`.
#[test]
fn a_read_at_a_name_is_typed_on_whether_it_is_a_number() {
    for declared in [":Number", ":Any"] {
        let (load, under, restarted) = indexed(declared);
        assert!(
            load[0].contains("ONE OF"),
            "the load names the read: {load:?}"
        );
        assert_eq!(under, Ok(restarted), "under `Item := {declared}`");
    }
}

/// A read at a name bound to a record of bounds is a slice: under `Item := {from :Number}`, `s`
/// and `t` are what a fresh load with `Item` declared gives them, whose `Ok` is the list alone.
#[test]
fn a_read_at_a_name_bound_to_a_record_of_bounds_is_what_a_fresh_load_reads() {
    let (_, under, restarted) = indexed(":{from :Number}");
    assert_eq!(under, Ok(restarted));
}

/// `SUBJECT`'s body: the module `m` holds `v`, a `MATCH` over `y :Item` whose type a binding
/// decides exact or not.
const SIGNS: &str = "#(SUBJECT y :Item) -> Any = #(\n  \
     MODULE m OVER #[y] = (LET v = (MATCH y WITH #{:Number: (1), _: (\"s\")}))\n  m\n)";

/// Each `MODULE` body signature the load lays, rendered.
fn signatures(door: &Door<'_, '_>) -> Vec<String> {
    let mut laid: Vec<String> = (door.load().own_units())
        .flat_map(|(_, (unit, _))| unit.typed.iter().filter_map(|(_, typed)| typed.signature))
        .map(|signature| display_name(signature, door.types, door.symbols()).to_string())
        .collect();
    laid.sort();
    laid.dedup();
    laid
}

/// Whether every member of a `MODULE` body is exact is a binding's to decide, so the body's
/// signature is laid at the load as the decided name on it: a binding of `Item` types the module
/// binder `m` as a fresh load with `Item` declared types it, at the signature where `v` is exact
/// and at `Never`'s where it is not.
#[test]
fn a_module_body_s_signature_is_laid_on_whether_its_members_are_exact() {
    for declared in [":Number", ":Any"] {
        let source = format!("LET Bound = {declared}\nEXPR FOR ALL #[Item] {SIGNS}");
        let laid = opened(&source, signatures);
        let ([load], under) = load_and_under(&source, "Item", ["m"]);
        let restarted = restarted(&format!("LET Item = {declared}\nEXPR {SIGNS}"), ["m"]);
        let [signature] = &laid[..] else {
            panic!("the load lays one signature: {laid:?}");
        };
        assert!(
            load.contains(signature.as_str()),
            "the load types `m` by the signature it lays over names: {load}"
        );
        assert_eq!(under, Ok(restarted), "under `Item := {declared}`");
    }
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
    let (spelled, under) = opened(&source, |door| {
        let f = subject(door.root());
        let outer = unknown(door.symbols(), f, "Outer");
        let under = (door.under(f, &[(outer, Bound::Type(KType::NUMBER))])).expect("no refusal");
        let derived = door.load().base_layer().derived();
        let spelled: Vec<String> = (derived.names().iter())
            .map(|name| display_name(name.ty, door.types, door.symbols()).to_string())
            .collect();
        (spelled, ["f", "t"].map(|name| ends(door, &under, f, name)))
    });
    let restarted = restarted(
        &format!("LET Outer = :Number\nEXPR {INSTANCES}"),
        ["f", "t"],
    );
    assert!(
        spelled.iter().any(|name| name.starts_with("(INSTANCE ")),
        "the load fixes the instance at a derived name: {spelled:?}"
    );
    assert_eq!(under, restarted);
}

/// A callable born in a quantified body reads the outer body's names: the use `PICK x` in `g`,
/// over `WRAP`'s `x :Elt`, keeps both overloads at the load and is selected under `Elt := Number`.
#[test]
fn an_inner_callable_over_an_outer_name_selects_under_its_binding() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
         LET g = (FN :{{y :Number}} -> Any = #(\n    LET p = (PICK x)\n    p\n  ))\n  g\n)"
    );
    let (load, under) = opened(&source, |door| {
        let f = quantified(door);
        let g = born(door, f, "g");
        let elt = unknown(door.symbols(), f, "Elt");
        let under = (door.under(g, &[(elt, Bound::Type(KType::NUMBER))])).expect("no refusal");
        (
            narrowing(door, door.load(), g, "p"),
            narrowing(door, &under, g, "p"),
        )
    });
    assert_eq!(load, "kept maybe maybe");
    assert_eq!(under, "selected");
}

/// A callable born inside a block reads the names of the body around the block: the use
/// `PICK x` in `g`, born in a `MATCH` arm, keeps both overloads at the load and is selected under
/// `Elt := Number`.
#[test]
fn an_inner_callable_inside_a_block_selects_under_its_binding() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
         MATCH 1 WITH #{{Number: ((LET g = (FN :{{y :Number}} -> Any = #(\
         (LET p = (PICK x)) (p)))) (g))}}\n)"
    );
    let (load, under) = opened(&source, |door| {
        let f = quantified(door);
        let (_, arm) = (f.nested_shapes().iter())
            .find(|(_, nested)| nested.kind() == ShapeKind::Block)
            .expect("the `MATCH` has an arm");
        let g = born(door, arm, "g");
        let elt = unknown(door.symbols(), f, "Elt");
        let under = (door.under(g, &[(elt, Bound::Type(KType::NUMBER))])).expect("no refusal");
        (
            narrowing(door, door.load(), g, "p"),
            narrowing(door, &under, g, "p"),
        )
    });
    assert_eq!(load, "kept maybe maybe");
    assert_eq!(under, "selected");
}

/// A binding under which no overload admits a use refuses it as a load with the binding declared
/// would: under `Elt := Str`, `x + 1` has no overload.
#[test]
fn a_binding_no_overload_admits_refuses_as_the_load_would() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET y = (x + 1)\n  y\n)";
    let refused = opened(source, |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        door.under(f, &[(elt, Bound::Type(KType::STR))]).map(|_| ())
    });
    let refused = refused.expect_err("no overload admits `Str`");
    assert!(
        refused.contains(
            "no overload of `_ + _` admits these arguments:\n  Str, which can never satisfy Number"
        ),
        "{refused}"
    );
}

/// A type binder written over a bound name declares closed under a binding of the name: `Box`
/// over `Elt` is unknown at the load and closed under `Elt := Number`.
#[test]
fn a_type_binder_over_a_bound_name_declares_closed() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
                  NEWTYPE Box = :{v :Elt}\n  (Box {v = x})\n)";
    let (load, under) = opened(source, |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let under = (door.under(f, &[(elt, Bound::Type(KType::NUMBER))])).expect("no refusal");
        (
            declares_closed(door, door.load(), f, "Box"),
            declares_closed(door, &under, f, "Box"),
        )
    });
    assert!(!load, "the load leaves the binder unknown");
    assert!(under, "the binding declares it closed");
}

/// An ascription the load proves fits at every run of its body is no longer proven under a
/// binding that drops the proof: `s :Elt = x` fits at the load, and under `Elt := Never` the
/// ascription holds no such proof.
#[test]
fn an_ascription_a_binding_no_longer_proves_holds_no_proof_under_it() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET s :Elt = x\n  s\n)";
    let (load, under) = opened(source, |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let under = (door.under(f, &[(elt, Bound::Type(KType::NEVER))])).expect("no refusal");
        (proven_in(door.load(), f, 0), proven_in(&under, f, 0))
    });
    assert_eq!((load, under), (1, 0));
}

/// A check the load leaves to the run is refused by a binding under which it never holds, and
/// holds trivially under one where the value never arrives: `x :! Key` refuses under
/// `Key := Never` with `Elt` at its bound `Any`, and is `Never` where `Elt := Never` too.
#[test]
fn a_binding_that_starves_a_refused_check_vacates_it() {
    let source = "EXPR FOR ALL #[Elt Key] #(WRAP x :Elt AND k :Key) -> Any = #(\n  \
                  LET s0 = (x :! Key)\n  x\n)";
    let (half, whole) = opened(source, |door| {
        let f = quantified(door);
        let (elt, key) = (
            unknown(door.symbols(), f, "Elt"),
            unknown(door.symbols(), f, "Key"),
        );
        let never = Bound::Type(KType::NEVER);
        let half = door.under(f, &[(elt, Bound::Type(KType::ANY)), (key, never)]);
        let whole = (door.under(f, &[(elt, never), (key, never)])).expect("no refusal");
        (half.map(|_| ()), upper(door, &whole, f, "s0"))
    });
    let refused = half.expect_err("binding `Key` to `Never` refuses the check");
    assert!(
        refused.contains("this value is Any, which can never satisfy its ascription Never"),
        "{refused}"
    );
    assert_eq!(whole, "Never");
}

/// A registration rigid over a `FOR ALL` name has its overlap checked under a binding that closes
/// it: under `Elt := Number` the `+` overload overlaps the builtin, and under `Elt := Str` it
/// overlaps none.
#[test]
fn a_binding_that_closes_a_registration_checks_its_overlap() {
    let under = |body: &str, bound: KType| {
        let source = format!(
            "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
             OP #(+) OVER Elt = #({body})\n  x\n)"
        );
        opened(&source, |door| {
            let f = quantified(door);
            let elt = unknown(door.symbols(), f, "Elt");
            door.under(f, &[(elt, Bound::Type(bound))]).map(|_| ())
        })
    };
    let refused = under("0", KType::NUMBER).expect_err("`Number` overlaps the builtin");
    assert!(
        refused.contains("this overload of `_ + _` takes operands the builtin"),
        "{refused}"
    );
    assert_eq!(under("x", KType::STR), Ok(()));
}

/// A binder over a list of a bound name only names it, judging nothing over it, and under a
/// binding of the name is the list of what it binds: `ys` over `xs :(LIST OF Elt)` is
/// `:(LIST OF Number)` under `Elt := Number`.
#[test]
fn a_binder_over_a_list_of_its_name_is_the_list_its_binding_closes() {
    let source = "EXPR FOR ALL #[Elt] #(WRAP xs :(LIST OF Elt)) -> Any = #(\n  \
                  LET ys = xs\n  ys\n)";
    let (named, typed) = opened(source, |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let named = waits_on(door, unit_of(door, f, "ys"), elt);
        let under = (door.under(f, &[(elt, Bound::Type(KType::NUMBER))])).expect("no refusal");
        (named, upper(door, &under, f, "ys"))
    });
    assert_eq!(
        named,
        Some(vec![None]),
        "`ys` names `Elt` and judges nothing over it"
    );
    assert_eq!(typed, ":(LIST OF Number)");
}

/// A parameter over a name binds its argument retyped to it, which a binding of the name closes:
/// under `Elt := Named` the argument `x` is keyed at `Named`, as a fresh load with `x :Named` keys
/// it, though the load could not say whether the retype lays a header.
#[test]
#[ignore = "`Door::under` reads `a`'s content tree off the load's record alone; a frame binds it from its run value"]
fn an_argument_retyped_to_a_name_is_retyped_as_the_binding_closes_it() {
    let body = |declared: &str, parameter: &str| {
        format!(
            "SIG Named = #[(VAL name :Str)]\nLET Sig = :Named\n\
             EXPR {declared} -> Any = #(\n  EXPR #(KEEP e :{parameter}) -> {parameter} = #(e)\n  \
             LET a = (KEEP x)\n  a\n)\n"
        )
    };
    let under = opened(&body("FOR ALL #[Elt] #(SUBJECT x :Elt)", "Elt"), |door| {
        let f = subject(door.root());
        let elt = unknown(door.symbols(), f, "Elt");
        let under =
            (door.under(f, &[(elt, Bound::Type(door.declared("Sig")))])).expect("no refusal");
        (under.content(f, slot(door.symbols(), f, "a"))).map(|tree| tree.identity())
    });
    let fresh = opened(&body("#(SUBJECT x :Named)", "Named"), |door| {
        let f = subject(door.root());
        (door.load().content(f, slot(door.symbols(), f, "a"))).map(|tree| tree.identity())
    });
    assert!(fresh.is_some());
    assert_eq!(under, fresh);
}

/// A field read of a variable is exact under a binding that makes it so: bound to `{f :Number}`,
/// `x` is exactly that record, so `x.f` is exactly `Number`, where the load gave
/// `[Never, (FIELD f OF Elt)]`.
#[test]
fn a_field_read_of_a_variable_is_exact_under_a_binding() {
    let source = "EXPR FOR ALL #{Elt: :{f :Number}} #(GET x :Elt) -> Any = #(\n  \
                  LET y = x.f\n  y\n)";
    let typed = opened(source, |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let field = BinderSymbol::declared("f", door.symbols()).expect("a value token");
        let record = door.types.record(door.bump, &[(field, KType::NUMBER)]);
        let under = (door.under(f, &[(elt, Bound::Type(record))])).expect("no refusal");
        ends(door, &under, f, "y")
    });
    assert_eq!(typed, "[Number, Number]");
}

/// `SUBJECT`'s body: `m` constructs `Meters` over `y :Item`, and `t` reads `m` alone.
const METERS: &str = "#(SUBJECT y :Item) -> Any = #(\n  LET m = (Meters y)\n  LET t = m\n  t\n)";

/// A construction of a closed newtype over a run-bound payload is `Meters` at most where the
/// payload may not fit, and exactly `Meters` under `Item := Number`, as a fresh load with the
/// binding declared types it.
#[test]
fn a_construction_over_a_run_bound_payload_is_what_a_fresh_load_types() {
    let declared = ":Number";
    let source =
        format!("LET Bound = {declared}\nNEWTYPE Meters = Number\nEXPR FOR ALL #[Item] {METERS}");
    let (_, under) = load_and_under(&source, "Item", ["m", "t"]);
    let restarted = restarted(
        &format!("NEWTYPE Meters = Number\nLET Item = {declared}\nEXPR {METERS}"),
        ["m", "t"],
    );
    assert_eq!(
        under,
        Ok(["[Meters, Meters]", "[Meters, Meters]"].map(String::from))
    );
    assert_eq!(under, Ok(restarted));
}

/// `SUBJECT`'s body: `s` calls `f :Fn` by name, and `t` reads `s` alone.
const CALLS: &str = "#(SUBJECT f :Fn) -> Any = #(\n  LET s = (f {x = 1})\n  LET t = s\n  t\n)";

/// A call through a head whose type is a run-bound name is typed by the call a binding gives it:
/// under `Fn := (FN :{x :Number} -> Number)` and `Fn := (FN :{x :Any} -> Str)`, `s` and `t` are
/// what a fresh load with `Fn` declared gives them.
#[test]
fn a_call_through_a_run_bound_head_is_what_a_fresh_load_types() {
    for declared in [":(FN :{x :Number} -> Number)", ":(FN :{x :Any} -> Str)"] {
        let source = format!(
            "LET Bound = {declared}\nEXPR FOR ALL #{{Fn: :(FN :{{x :Number}} -> Any)}} {CALLS}"
        );
        let (_, under) = load_and_under(&source, "Fn", ["s", "t"]);
        let restarted = restarted(&format!("LET Fn = {declared}\nEXPR {CALLS}"), ["s", "t"]);
        assert_eq!(under, Ok(restarted), "under `Fn := {declared}`");
    }
}

/// `SUBJECT`'s body: `s` calls the closed `g` by name over `p :Arg`, and `t` reads `s` alone.
const LAYS: &str = "#(SUBJECT p :Arg) -> Any = #(\n  \
                    LET g = (FN :{x :Number} -> Number = #(x))\n  \
                    LET s = (g p)\n  LET t = s\n  t\n)";

/// A call by name whose argument's type is a run-bound name lays it out by the rule a binding
/// closes: under `Arg := {x :Number}`, `s` and `t` are what a fresh load with `Arg` declared gives
/// them, and under `Arg := {x :Number y :Str}`, which names a field `g` does not declare, the
/// binding refuses the call as a fresh load with `Arg` declared refuses it.
#[test]
fn a_call_over_a_run_bound_argument_is_what_a_fresh_load_lays_out() {
    let source =
        |declared: &str| format!("LET Bound = {declared}\nEXPR FOR ALL #{{Arg: :Any}} {LAYS}");
    let (_, under) = load_and_under(&source(":{x :Number}"), "Arg", ["s", "t"]);
    let restarted = restarted(
        &format!("LET Arg = :{{x :Number}}\nEXPR {LAYS}"),
        ["s", "t"],
    );
    assert_eq!(under, Ok(restarted), "under `Arg := {{x :Number}}`");
    let (_, refused) = load_and_under(&source(":{x :Number y :Str}"), "Arg", ["s", "t"]);
    let refused = refused.expect_err("the binding refuses the call");
    assert!(
        refused.contains("can never satisfy the parameters"),
        "{refused}"
    );
    let fresh = super::run(&format!("LET Arg = :{{x :Number y :Str}}\nEXPR {LAYS}"));
    let fresh = (fresh.strip_prefix("load: ")).expect("the fresh load refuses");
    assert_eq!(unlocated(&refused), unlocated(fresh));
}

/// `Counter`, and `hidden`: `ints` sealed behind it, whose carrier is opaque.
const HIDDEN: &str = r#"SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]
MODULE ints = (
  LET zero = 0
  LET succ = (FN :{x :Number} -> Number = #(x + 1))
)
LET hidden = (ints :| Counter)
"#;

/// `BOTH`'s body: `Both` meets `Elt` with `hidden`'s opaque carrier.
const BOTH: &str = r#"#(BOTH x :Elt) -> Any = #(
  LET Both = :(Elt & hidden.Carrier)
  x
)
PRINT (BOTH 1)
"#;

/// A meet of a run-bound name with an opaque carrier is a derived name whose ends are widened past
/// the carrier, so the load types it and nothing holds; under `Elt := Number` it is what a fresh
/// load with `Elt` declared gives it.
#[test]
fn a_meet_with_an_opaque_carrier_is_a_derived_name_a_binding_closes() {
    let (load, under) = opened(&format!("{HIDDEN}EXPR FOR ALL #[Elt] {BOTH}"), |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let under = (door.under(f, &[(elt, Bound::Type(KType::NUMBER))])).expect("no refusal");
        (
            declared_as(door, door.load(), f, "Both"),
            declared_as(door, &under, f, "Both"),
        )
    });
    let restarted = opened(&format!("{HIDDEN}LET Elt = Number\nEXPR {BOTH}"), |door| {
        declared_as(door, door.load(), subject(door.root()), "Both")
    });
    let load = load.expect("the load types `Both`");
    assert!(load.starts_with('('), "a derived name: {load}");
    assert!(under.is_some(), "the binding types `Both`");
    assert_eq!(under, restarted);
}

/// `SUBJECT`'s body: `Node` and its alias fellow `Children`, which reads `Elt`.
const NODE: &str = "#(SUBJECT x :Elt) -> Any = #(\n  \
                    NEWTYPE Node = :{kids :Children}\n  \
                    LET Children = :(LIST OF (Node | Elt))\n  \
                    x\n)\n";

/// An alias fellow of a nominal reading a run-bound name is rigid and sealed with the group: each
/// member is a derived name the load types and nothing holds, and under `Elt := Number` each is
/// what a fresh load with `Elt` declared gives it.
#[test]
fn an_alias_fellow_of_a_nominal_over_a_run_bound_name_seals_with_the_group() {
    let (load, under) = opened(&format!("EXPR FOR ALL #[Elt] {NODE}"), |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let under = (door.under(f, &[(elt, Bound::Type(KType::NUMBER))])).expect("no refusal");
        (
            ["Node", "Children"].map(|name| declared_as(door, door.load(), f, name)),
            ["Node", "Children"].map(|name| declared_as(door, &under, f, name)),
        )
    });
    let restarted = opened(&format!("LET Elt = Number\nEXPR {NODE}"), |door| {
        let f = subject(door.root());
        ["Node", "Children"].map(|name| declared_as(door, door.load(), f, name))
    });
    for typed in &load {
        let typed = typed.as_deref().expect("the load types each member");
        assert!(typed.starts_with('('), "a derived name: {typed}");
    }
    assert!(under.iter().all(Option::is_some), "{under:?}");
    assert_eq!(under, restarted);
}
