//! Body classes: the class the load gives each frame body — sealed or porous by its exits — and the
//! sites it mints at at run, the blocks and module bodies it runs among them; and the caching record
//! each shape carries — each minting site closed, invariant or varying, and each warm composite.

use crate::program::Program;
use crate::scope::{BodyClass, BodyShape, Mint, ShapeKind};

use super::run;
use super::statics::{body, loaded, module_body};
use crate::values::BuildBudget;

/// `SIG Counter`, whose head parameter an opaque view hides, and `MODULE ints`, which fits it.
const COUNTER: &str = "SIG Counter FOR ALL #[Carrier] = \
                       #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                       MODULE ints = ((LET zero = 0) \
                       (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n";

/// A class, rendered: `sealed` or `porous`, the sites it mints at at run, and `caches` where a
/// sealed body caches.
fn rendered(class: Option<BodyClass>) -> String {
    match class.expect("a frame body has a class") {
        BodyClass::Sealed { mints, caches } => {
            format!("sealed {mints}{}", if caches { " caches" } else { "" })
        }
        BodyClass::Porous { mints } => format!("porous {mints}"),
    }
}

/// The class of the body the top-level binder `name` of `COUNTER` and `source` births.
fn class(source: &str, name: &str) -> String {
    loaded(&format!("{COUNTER}{source}"), |program| {
        rendered(program.facts().class(body(program, program.shape(), name)))
    })
}

/// The class of the one quote's code nested in the top level of `COUNTER` and `source`.
fn code_class(source: &str) -> String {
    loaded(&format!("{COUNTER}{source}"), |program: &Program<'_>| {
        let (_, code) = (program.shape().nested_shapes().iter())
            .find(|(_, nested)| nested.kind() == ShapeKind::Code)
            .expect("the program writes a quote");
        rendered(program.facts().class(code))
    })
}

#[test]
fn a_body_returning_no_view_is_sealed_and_caches_whatever_its_placement() {
    let body = "(LET v = (m :| Counter)) (c)";
    assert_eq!(
        class(
            &format!("LET f = (FN :{{m :Counter, c :Number}} -> Any = #({body}))"),
            "f"
        ),
        "sealed 1 caches",
        "the return is porous, the value returned is not"
    );
    assert_eq!(
        class(
            &format!("LET f = (FN :{{m :Counter, c :Number}} -> Number = #({body}))"),
            "f"
        ),
        "sealed 1 caches",
        "a body placed fresh caches too"
    );
}

#[test]
fn a_body_returning_its_view_is_porous() {
    assert_eq!(
        class(
            "LET f = (FN :{m :Counter, c :Number} -> Any = #((LET v = (m :| Counter)) (v)))",
            "f"
        ),
        "porous 1"
    );
}

#[test]
fn a_call_into_a_frame_at_a_porous_slot_is_an_exit_and_a_builtin_is_none() {
    let keep = "EXPR #(KEEP x :Any) -> Number = #(1)\n";
    assert_eq!(
        class(
            &format!(
                "{keep}LET f = (FN :{{m :Counter, c :Number}} -> Any = \
                 #((LET v = (m :| Counter)) (KEEP v) (c)))"
            ),
            "f"
        ),
        "porous 1"
    );
    assert_eq!(
        class(
            "LET f = (FN :{m :Counter, c :Number} -> Any = #((LET v = (m :| Counter)) (PRINT v) (c)))",
            "f"
        ),
        "sealed 1 caches",
        "a builtin's native enters no frame"
    );
    assert_eq!(
        class(
            &format!(
                "{keep}LET f = (FN :{{m :Counter, c :Number}} -> Any = \
                 #((LET v = (m :| Counter)) (KEEP c) (c)))"
            ),
            "f"
        ),
        "sealed 1 caches",
        "a value that is not porous carries nothing out"
    );
}

#[test]
fn a_call_by_name_is_an_exit_where_a_porous_value_meets_a_porous_slot() {
    let view = "(LET v = (m :| Counter))";
    assert_eq!(
        class(
            &format!(
                "LET g = (FN :{{x :Any}} -> Number = #(1))\n\
                 LET f = (FN :{{m :Counter, c :Number}} -> Number = #({view} (g {{x = v}}) (c)))"
            ),
            "f"
        ),
        "porous 1",
        "an exact callee whose parameter is porous"
    );
    assert_eq!(
        class(
            &format!(
                "LET g = (FN :{{x :Any}} -> Number = #(1))\n\
                 LET f = (FN :{{m :Counter, c :Number}} -> Number = #({view} (g {{x = c}}) (c)))"
            ),
            "f"
        ),
        "sealed 1 caches",
        "a value that is not porous carries nothing out, at a porous slot too"
    );
    assert_eq!(
        class(
            &format!(
                "LET f = (FN :{{m :Counter, c :Number, h :(FN :{{x :Any}} -> Number)}} -> Number = \
                 #({view} (h v) (c)))"
            ),
            "f"
        ),
        "porous 1",
        "a callee known at most"
    );
    assert_eq!(
        class(
            &format!(
                "LET f = (FN :{{m :Counter, c :Number, h :Any}} -> Number = #({view} (h v) (c)))"
            ),
            "f"
        ),
        "porous 1",
        "a callee the load cannot type"
    );
}

#[test]
fn a_value_the_body_received_passed_on_is_no_exit() {
    let keep = "EXPR #(KEEP x :Any) -> Number = #(1)\n";
    let view = "(LET v = (m :| Counter))";
    let passing = |call: &str| {
        class(
            &format!(
                "{keep}LET g = (FN :{{x :Any}} -> Number = #(1))\n\
                 LET f = (FN :{{m :Counter, c :Number, h :Any}} -> Number = #({view} {call} (c)))"
            ),
            "f",
        )
    };
    assert_eq!(passing("(KEEP m)"), "sealed 1 caches", "a keyworded use");
    assert_eq!(passing("(g {x = m})"), "sealed 1 caches", "a call by name");
    assert_eq!(
        passing("(h m)"),
        "sealed 1 caches",
        "a callee the load cannot type"
    );
    assert_eq!(
        passing("(g {x = v})"),
        "porous 1",
        "what the body minted still leaves"
    );
    assert_eq!(
        passing("(g {x = m.zero})"),
        "sealed 1 caches",
        "a member of one"
    );
    assert_eq!(
        passing("(g {x = v.zero})"),
        "porous 1",
        "and a member of it"
    );
    let source = "LET g = (FN :{x :Any} -> Number = #(1))\n\
                  LET mk = (FN :{m :Counter} -> Any = #(\
                  (LET h = (FN :{n :Number} -> Number = #((LET v = (m :| Counter)) (g {x = m}) (n)))) \
                  (h)))";
    loaded(&format!("{COUNTER}{source}"), |program| {
        let mk = body(program, program.shape(), "mk");
        assert!(record_of(program, body(program, mk, "h")).1, "a capture");
    });
}

#[test]
fn a_let_bound_bare_to_a_received_value_passed_on_is_no_exit() {
    let keep = "EXPR #(KEEP x :Any) -> Number = #(1)\n";
    let view = "(LET v = (m :| Counter))";
    let passing = |call: &str| {
        class(
            &format!(
                "{keep}MODULE top = ((LET zero = [{{x = 1}}, 1]))\n\
                 LET f = (FN :{{m :Counter, c :Number}} -> Number = #({view} {call} (c)))"
            ),
            "f",
        )
    };
    assert_eq!(
        passing("(LET a = m) (KEEP a)"),
        "sealed 1 caches",
        "a parameter"
    );
    assert_eq!(
        passing("(LET a = m) (LET b = a) (KEEP b.zero)"),
        "sealed 1 caches",
        "a member of a `LET` bound to one"
    );
    assert_eq!(
        passing("(LET a = top) (KEEP a.zero)"),
        "sealed 1 caches",
        "a capture"
    );
    assert_eq!(
        passing("(LET a = v) (KEEP a)"),
        "porous 1",
        "what the body minted still leaves"
    );
}

/// `SIG Holder`, `PEEK` and `KEEP`, registrations at a signature-typed and an `Any` slot, and `g`
/// and `r`, functions at the same.
const PASSERS: &str = "SIG Holder FOR ALL #[Elt] = #[(VAL held :Elt)]\n\
                       EXPR #(PEEK h :Holder) -> Number = #(1)\n\
                       EXPR #(KEEP x :Any) -> Number = #(1)\n\
                       LET g = (FN :{x :Holder} -> Number = #(1))\n\
                       LET r = (FN :{x :Any} -> Number = #(1))\n";

/// The class of `f`, a body viewing its parameter `m`, then running `statements`.
fn passing(statements: &str) -> String {
    class(
        &format!(
            "{PASSERS}LET f = (FN :{{m :Counter, c :Number, k :Any}} -> Number = \
             #((LET v = (m :| Counter)) {statements} (c)))"
        ),
        "f",
    )
}

#[test]
fn a_module_built_over_what_the_body_received_is_no_exit() {
    let pack = "(MODULE pack OVER #[c] = (LET held = c))";
    assert_eq!(passing(&format!("{pack} (PEEK pack)")), "sealed 1 caches");
    assert_eq!(
        passing(&format!("{pack} (g {{x = pack}})")),
        "sealed 1 caches",
        "a call by name"
    );
    assert_eq!(
        passing(&format!("{pack} (k pack)")),
        "sealed 1 caches",
        "a callee the load cannot type"
    );
}

#[test]
fn a_module_capturing_what_the_body_minted_is_an_exit() {
    assert_eq!(
        passing("(MODULE pack OVER #[v] = (LET held = v)) (PEEK pack)"),
        "porous 1"
    );
    assert_eq!(
        passing("(MODULE pack OVER #[m] = (LET held = (m :| Counter))) (PEEK pack)"),
        "porous 2",
        "the module body mints"
    );
    assert_eq!(
        loaded(
            &format!(
                "{COUNTER}{PASSERS}LET v = (ints :| Counter)\n\
                 MODULE pack = (LET held = v)\nPRINT (PEEK pack)"
            ),
            |program| rendered(program.facts().class(program.shape()))
        ),
        "porous 0",
        "a module reading the program's own binder, where the program is the frame body"
    );
}

#[test]
fn a_module_capturing_a_local_is_read_through_it() {
    let peeked = |built: &str| passing(&format!("{built} (PEEK pack)"));
    assert_eq!(
        peeked("(LET d = c) (MODULE pack OVER #[d] = (LET held = d))"),
        "sealed 1 caches",
        "an alias of a parameter"
    );
    assert_eq!(
        peeked("(LET d = 5) (MODULE pack OVER #[d] = (LET held = d))"),
        "sealed 1 caches",
        "a literal"
    );
    assert_eq!(
        peeked(
            "(MODULE inner OVER #[c] = (LET held = c)) \
             (MODULE pack OVER #[inner] = (LET held = inner))"
        ),
        "sealed 1 caches",
        "a module built over a parameter"
    );
    assert_eq!(
        peeked(
            "(MODULE inner OVER #[v] = (LET held = v)) \
             (MODULE pack OVER #[inner] = (LET held = inner))"
        ),
        "porous 1",
        "a module capturing a view"
    );
    assert_eq!(
        peeked("(LET d = [c]) (MODULE pack OVER #[d] = (LET held = d))"),
        "porous 1",
        "a local the load cannot see through"
    );
    // A quantified function is bound as a module member, where nothing need fix its group.
    let source = format!(
        "{PASSERS}MODULE lib = (LET f = (FN FOR ALL #[Elt] :{{m :Counter, x :Elt}} -> Number = \
         #((LET v = (m :| Counter)) (MODULE pack OVER #[x, Elt] = (LET held :Elt = x)) \
         (PEEK pack) (1))))"
    );
    loaded(&format!("{COUNTER}{source}"), |program| {
        let f = body(program, module_body(program, "lib"), "f");
        assert_eq!(
            rendered(program.facts().class(f)),
            "sealed 1 caches",
            "a type capture of the body's own variable"
        );
    });
}

#[test]
fn a_record_written_in_place_is_judged_field_by_field() {
    assert_eq!(passing("(KEEP {a = c, b = m})"), "sealed 1 caches");
    assert_eq!(passing("(KEEP {a = {b = m}})"), "sealed 1 caches", "nested");
    assert_eq!(
        passing("(KEEP {a = 5, b = m})"),
        "sealed 1 caches",
        "a field that is not porous"
    );
    assert_eq!(passing("(KEEP {a = v})"), "porous 1");
    assert_eq!(
        passing("(r {x = {a = m}})"),
        "sealed 1 caches",
        "a call by name, a literal nested"
    );
    assert_eq!(
        passing("(LET q = {a = m}) (KEEP q)"),
        "porous 1",
        "a record bound by `LET` is read by its static type"
    );
}

#[test]
fn a_return_holds_nothing_minted_by_the_same_rule() {
    let returning = |returns: &str, last: &str| {
        class(
            &format!(
                "LET f = (FN :{{m :Counter, c :Number}} -> {returns} = \
                 #((LET v = (m :| Counter)) {last}))"
            ),
            "f",
        )
    };
    assert_eq!(returning("Counter", "(m)"), "sealed 1 caches", "received");
    assert_eq!(
        returning("Any", "(m.zero)"),
        "sealed 1 caches",
        "a builtin member read of a received value"
    );
    assert_eq!(
        returning("Any", "(MODULE pack OVER #[c] = (LET held = c))"),
        "sealed 1 caches",
        "a binding last statement"
    );
    assert_eq!(
        returning("Any", "(MODULE pack OVER #[c] = (LET held = c)) (pack)"),
        "sealed 1 caches"
    );
    assert_eq!(returning("Any", "({a = c, b = m})"), "sealed 1 caches");
    assert_eq!(returning("Any", "({a = 5, b = m})"), "sealed 1 caches");
    assert_eq!(
        returning("Any", "(MODULE pack OVER #[v] = (LET held = v)) (pack)"),
        "porous 1"
    );
}

#[test]
fn every_eval_is_an_exit() {
    assert_eq!(
        class(
            "LET f = (FN :{m :Counter, c :Number} -> Number = \
             #((LET v = (m :| Counter)) (EVAL #(1) -> Number) (c)))",
            "f"
        ),
        "porous 1"
    );
}

#[test]
fn a_module_body_counts_toward_the_body_running_it() {
    assert_eq!(
        class(
            "LET f = (FN :{m :Counter, c :Number} -> Number = \
             #((MODULE inner OVER #[m] = (LET v = (m :| Counter))) (c)))",
            "f"
        ),
        "sealed 1 caches"
    );
}

#[test]
fn a_call_result_returned_is_the_only_exit() {
    assert_eq!(
        class(
            "LET g = (FN :{x :Number} -> Any = #(x))\n\
             LET f = (FN :{m :Counter, c :Number} -> Any = #((LET v = (m :| Counter)) (g {x = c})))",
            "f"
        ),
        "porous 1"
    );
}

#[test]
fn a_nominal_declared_per_call_mints() {
    // A quantified function is bound as a module member, where nothing need fix its group.
    let source = "MODULE lib = (LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Number = \
                  #((NEWTYPE Box = :{v :Elt}) (1))))";
    loaded(&format!("{COUNTER}{source}"), |program| {
        let f = body(program, module_body(program, "lib"), "f");
        assert_eq!(rendered(program.facts().class(f)), "sealed 1 caches");
    });
    assert_eq!(
        class(
            "LET f = (FN :{x :Number} -> Number = #((NEWTYPE Box = :{v :Number}) (1)))",
            "f"
        ),
        "sealed 0",
        "a nominal the load closes is declared once"
    );
}

#[test]
fn an_arm_counts_toward_the_body_running_it() {
    assert_eq!(
        class(
            "LET f = (FN :{m :Counter, c :Number} -> Number = \
             #(MATCH (c) WITH #{Number: ((LET v = (m :| Counter)) (it))}))",
            "f"
        ),
        "sealed 1 caches"
    );
    assert_eq!(
        class(
            "EXPR #(KEEP x :Any) -> Number = #(1)\n\
             LET f = (FN :{m :Counter, c :Number} -> Number = \
             #((LET v = (m :| Counter)) (MATCH (c) WITH #{Number: (KEEP v)})))",
            "f"
        ),
        "porous 1"
    );
}

#[test]
fn a_nested_function_is_a_body_of_its_own() {
    let source = "LET f = (FN :{m :Counter, c :Number} -> Any = #(\
                  (LET h = (FN :{m :Counter, y :Number} -> Number = #((LET v = (m :| Counter)) (y)))) \
                  (c)))";
    assert_eq!(class(source, "f"), "sealed 0");
    loaded(&format!("{COUNTER}{source}"), |program| {
        let f = body(program, program.shape(), "f");
        assert_eq!(
            rendered(program.facts().class(body(program, f, "h"))),
            "sealed 1 caches"
        );
    });
}

#[test]
fn a_view_leaves_through_each_porous_return() {
    let returning = |returns: &str, value: &str| {
        class(
            &format!(
                "NEWTYPE Box = :{{v :Any}}\n\
                 LET f = (FN :{{m :Counter, c :Number}} -> {returns} = \
                 #((LET v = (m :| Counter)) ({value})))"
            ),
            "f",
        )
    };
    assert_eq!(returning("Value", "[v]"), "porous 1");
    assert_eq!(returning("Module", "v"), "porous 1");
    assert_eq!(returning(":{b :Number}", "{a = v, b = 1}"), "porous 1");
    assert_eq!(returning("Type", "v.Carrier"), "porous 1");
    assert_eq!(returning("Box", "Box {v = v}"), "porous 1");
    assert_eq!(
        returning(":{x :(:{b :Number} | Null)}", "{x = {a = v, b = 1}}"),
        "porous 1",
        "under a union a part keeps its own memo"
    );
    assert_eq!(
        returning(":{b :Number}", "{b = 1}"),
        "sealed 1 caches",
        "an exact closed record exposes only what it names"
    );
}

#[test]
fn the_program_and_a_quote_are_frame_bodies() {
    assert_eq!(
        loaded(
            &format!("{COUNTER}LET v = (ints :| Counter)\nPRINT v"),
            |program| rendered(program.facts().class(program.shape()))
        ),
        "sealed 0",
        "a view over a closed tree mints nothing at run"
    );
    assert_eq!(
        code_class("LET code = #((LET v = (ints :| Counter)) (1))"),
        "sealed 1 caches",
        "a quote's code reads a capture across its root as a leaf"
    );
    assert_eq!(
        code_class("LET code = #((LET v = (ints :| Counter)) (v))"),
        "porous 1"
    );
}

/// The caching record of `shape`: each own minting site's class in site order, and whether its
/// frame caches.
fn record_of(program: &Program<'_>, shape: &BodyShape<'_>) -> (Vec<Mint>, bool) {
    let caching = (program.facts().caching(shape)).expect("the load fixed the record");
    let mints = caching
        .mints
        .entries()
        .iter()
        .map(|(_, mint)| *mint)
        .collect();
    (mints, caching.caches)
}

/// [`record_of`] the body the top-level binder `name` of `COUNTER` and `source` births.
fn caching(source: &str, name: &str) -> (Vec<Mint>, bool) {
    loaded(&format!("{COUNTER}{source}"), |program| {
        record_of(program, body(program, program.shape(), name))
    })
}

#[test]
fn a_closed_view_mints_nothing_at_run() {
    assert_eq!(
        caching(
            "LET f = (FN :{c :Number} -> Any = #((LET v = (ints :| Counter)) (c)))",
            "f"
        ),
        (vec![Mint::Closed], false)
    );
}

#[test]
fn a_view_over_a_parameter_a_tail_loop_passes_on_is_invariant() {
    let looping = |passed: &str| {
        caching(
            &format!(
                "LET loop = (FN :{{m :Counter, n :Number}} -> Number = \
                 #((LET v = (m :| Counter)) (loop {{m = {passed}, n = n}})))"
            ),
            "loop",
        )
    };
    // Passing the module on bare is no exit, so the loop caches.
    assert_eq!(looping("m"), (vec![Mint::Invariant], true));
    assert_eq!(
        looping("ints").0,
        [Mint::Varying],
        "a parameter rebound on a tail edge varies"
    );
}

#[test]
fn a_keyworded_tail_loop_pairs_its_arguments_by_slot() {
    let looping = |passed: &str| {
        let source = format!(
            "NEWTYPE Link = :{{next :Any}}\n\
             EXPR #(COUNTDOWN m :Counter link :Link) -> Number = \
             #((LET v = (m :| Counter)) (COUNTDOWN {passed} (ATTR link next)))\n\
             EXPR #(COUNTDOWN m :Counter end :Null) -> Number = #(0)"
        );
        loaded(&format!("{COUNTER}{source}"), |program| {
            let (_, looping) = (program.shape().nested_shapes().iter())
                .find(|(_, nested)| {
                    nested.kind() == ShapeKind::Callable
                        && (program.facts().caching(nested))
                            .is_some_and(|c| !c.mints.entries().is_empty())
                })
                .expect("the looping registration births a callable body");
            record_of(program, looping)
        })
    };
    // A member of a received value is no exit either, so the loop caches.
    assert_eq!(looping("m"), (vec![Mint::Invariant], true));
    assert_eq!(looping("ints").0, [Mint::Varying], "a slot rebound varies");
}

#[test]
fn a_callable_read_as_a_value_keeps_its_parameters_varying() {
    assert_eq!(
        caching(
            "LET h = (FN :{m :Counter} -> Number = #((LET v = (m :| Counter)) (1)))\n\
             LET k = h",
            "h"
        ),
        (vec![Mint::Varying], true)
    );
}

#[test]
fn a_view_over_a_local_binder_varies() {
    assert_eq!(
        caching(
            "LET f = (FN :{n :Number} -> Number = \
             #((LET w = (MATCH (n) WITH #{Number: (ints)})) (LET v = (w :| Counter)) (n)))",
            "f"
        )
        .0,
        [Mint::Varying]
    );
}

#[test]
fn a_view_over_a_capture_is_invariant() {
    let source = "LET mk = (FN :{m :Counter} -> Any = #(\
                  (LET h = (FN :{n :Number} -> Number = #((LET v = (m :| Counter)) (n)))) \
                  (h)))";
    loaded(&format!("{COUNTER}{source}"), |program| {
        let mk = body(program, program.shape(), "mk");
        assert_eq!(
            record_of(program, body(program, mk, "h")),
            (vec![Mint::Invariant], true)
        );
    });
}

#[test]
fn a_view_over_a_view_follows_the_parameter_under_it() {
    assert_eq!(
        caching(
            "LET f = (FN :{m :Counter} -> Number = #((LET v = ((m :| Counter) :| Counter)) (1)))",
            "f"
        ),
        (vec![Mint::Invariant, Mint::Invariant], true)
    );
}

#[test]
fn a_nominal_declared_per_call_follows_the_type_it_reads() {
    let source = "EXPR FOR ALL #[Elt] #(BOXED x :Elt) -> Number = \
                  #((NEWTYPE Box = :{v :Elt}) (1))";
    loaded(&format!("{COUNTER}{source}"), |program| {
        let (_, registered) = (program.shape().nested_shapes().iter())
            .find(|(_, nested)| nested.kind() == ShapeKind::Callable)
            .expect("the registration births a callable body");
        assert_eq!(
            record_of(program, registered),
            (vec![Mint::Invariant], true)
        );
    });
}

/// Debug builds check at each minting door that the load recorded the site: a view and a nominal
/// declared per call, each in a callable's body, run without tripping either check.
#[test]
fn the_run_finds_each_minting_site_the_load_recorded() {
    assert_eq!(
        run(&format!(
            "{COUNTER}MODULE lib = (LET f = (FN FOR ALL #[Elt] :{{x :Elt}} -> Number = \
             #((NEWTYPE Box = :{{v :Elt}}) (LET v = (ints :| Counter)) (1))))\n\
             PRINT (lib.f {{x = 2}})"
        )),
        "1"
    );
}

/// Each runnable snippet of the tutorial, as `tools/verify_snippets.py` picks them: a `koan` block
/// followed, with only whitespace between, by a `text` block, and holding no pending expression
/// shape.
pub(super) fn tutorial_snippets() -> Vec<(String, String)> {
    const PENDING: [&str; 6] = ["MATCH", "TRY", "CATCH", "Result", "TYPE", "CLOSE"];
    let pending = |code: &str| {
        code.split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|word| PENDING.contains(&word))
    };
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tutorial");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("the tutorial is readable")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    files.sort();
    let mut snippets = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path).expect("a tutorial page is readable");
        // Each fenced block: its language, its body, and the text after it up to the next fence.
        let mut blocks = Vec::new();
        let mut rest = text.as_str();
        while let Some(open) = rest.find("```") {
            let after = &rest[open + 3..];
            let Some(newline) = after.find('\n') else {
                break;
            };
            let language = &after[..newline];
            let Some(close) = after[newline + 1..].find("\n```") else {
                break;
            };
            let content = &after[newline + 1..newline + 1 + close];
            rest = &after[newline + 1 + close + 4..];
            blocks.push((language, content, rest.find("```").map(|at| &rest[..at])));
        }
        for pair in blocks.windows(2) {
            let ((language, code, between), (next, _, _)) = (pair[0], pair[1]);
            if language == "koan"
                && next == "text"
                && between.is_some_and(|gap| gap.trim().is_empty())
                && !pending(code)
            {
                let name = path.file_name().expect("a file").to_string_lossy();
                snippets.push((name.into_owned(), code.to_string()));
            }
        }
    }
    snippets
}

/// The measurement the type-tiers work reads: over the tutorial's runnable programs, how many
/// frame bodies the load gives each class, and how many sites minting at run each caching body
/// holds. Run with `cargo test classes_over_the_tutorial -- --ignored --nocapture`.
#[test]
#[ignore]
fn classes_over_the_tutorial() {
    use std::collections::BTreeMap;
    use std::collections::HashSet;

    use crate::program::CellSubstrate;
    use crate::scope::BodyShape;

    use super::{Koan, output};

    let (mut programs, mut refused) = (0, 0);
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut caching: BTreeMap<u32, usize> = BTreeMap::new();
    for (_, code) in tutorial_snippets() {
        let Ok(mut substrate) =
            CellSubstrate::load::<Koan>(&code, "<tutorial>", 8, BuildBudget::default(), output())
        else {
            refused += 1;
            continue;
        };
        programs += 1;
        substrate.with(|running| {
            let mut seen = HashSet::new();
            let mut stack: Vec<&BodyShape<'_>> = vec![running.program().shape()];
            while let Some(shape) = stack.pop() {
                if !seen.insert(shape as *const BodyShape<'_>) {
                    continue;
                }
                stack.extend(shape.nested_shapes().iter().map(|(_, nested)| *nested));
                let kind = match shape.kind() {
                    ShapeKind::Program => "program",
                    ShapeKind::Callable => "callable",
                    ShapeKind::Code => "code",
                    ShapeKind::Block | ShapeKind::Module => continue,
                };
                let class = match running.program().facts().class(shape) {
                    None => "unfixed".to_string(),
                    Some(BodyClass::Porous { .. }) => "porous".to_string(),
                    Some(BodyClass::Sealed { mints, caches }) => {
                        if caches {
                            *caching.entry(mints).or_default() += 1;
                        }
                        match (mints, caches) {
                            (0, _) => "sealed, mints nothing at run",
                            (_, true) => "sealed, caches",
                            (_, false) => "sealed, mints, caches nothing",
                        }
                        .to_string()
                    }
                };
                *kinds.entry(format!("{kind:8} {class}")).or_default() += 1;
            }
        });
    }
    println!("{programs} tutorial programs loaded, {refused} refused at load");
    println!("| frame body | class | bodies |\n|---|---|---|");
    for (row, count) in &kinds {
        let (kind, class) = row.split_at(8);
        println!("| {} | {} | {count} |", kind.trim(), class.trim());
    }
    println!("\n| sites minting at run | caching bodies |\n|---|---|");
    for (mints, count) in &caching {
        println!("| {mints} | {count} |");
    }
}
