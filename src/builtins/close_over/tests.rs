//! `CLOSE OVER` acceptance. Severance is a claim about *reach*, so it is observed through the
//! memory substrate rather than through what a read returns: `region_metrics().live` is sampled
//! while the escaped value is still held, and the discriminator is how that count moves as the
//! producer chain gets deeper. A severed closure's count is flat — it pins its own block region and
//! nothing else — where the same closure written plainly pins one more region per enclosing
//! per-call frame.
//!
//! The producer chains are built from **anonymous** `FN`s. A keyworded `FN` inside a per-call frame
//! is a dispatch registration, which implicit close pins on purpose; an anonymous one is a plain
//! value binding, so a chain of them isolates the lexical-frame retention the form exists to cut.

mod consolidation;
mod inferred;

use crate::builtins::test_support::TestRun;
use crate::machine::KErrorKind;
use crate::machine::core::KoanRegionExt;
use crate::machine::model::KObject;
use crate::machine::{program_storage, run_root_storage};
use crate::witnessed::{region_metrics, reset_region_metrics};

/// Run `source` and hand back everything it printed.
fn output(source: &str) -> String {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);
    test_run.run(source);
    String::from_utf8(captured.borrow().clone()).expect("output is utf8")
}

/// Run `prelude`, then dispatch `probe` and hand back the `KError` it raised.
fn error_of(prelude: &str, probe: &str) -> crate::machine::KError {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    if !prelude.is_empty() {
        test_run.run(prelude);
    }
    test_run.run_one_err(test_run.parse_one(probe))
}

/// Regions live at the moment `source` has finished **while the run is still held** — the window in
/// which an escaped value's reach is what keeps regions alive — paired with the count after the
/// whole run drops. The program storage is minted into the baseline exactly as
/// `fn_def::tests::region_liveness` does: it is alive on both sides of the measurement.
fn held_and_released(source: &str) -> (usize, usize) {
    reset_region_metrics();
    let program = program_storage();
    program.brand();
    let baseline = region_metrics().live;
    let held = {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(source);
        region_metrics().live - baseline
    };
    (held, region_metrics().live - baseline)
}

/// A producer chain `depth` anonymous frames deep, ending in `inner`, whose result escapes to the
/// run root as `esc`. Every level is a per-call frame whose lexical parent is the level above, so a
/// closure built at the bottom and written plainly pins the whole run of them.
fn producer_chain(depth: usize, inner: &str, trailer: &str) -> String {
    let mut body = inner.to_string();
    for level in (1..=depth).rev() {
        body = format!("(LET f{level} = (FN :{{}} -> Any = ({body}))) (f{level} {{}})");
    }
    format!("LET f0 = (FN :{{n :Number}} -> Any = ({body}))\nLET esc = (f0 {{n = 7}})\n{trailer}")
}

/// The closure the chains escape: a thunk reading whatever the block bound.
const THUNK: &str = "(LET g = (FN :{} -> Number = (n))) (g)";

// ---------- the block itself ----------

#[test]
fn an_empty_capture_list_runs_the_block() {
    assert_eq!(output("LET x = (CLOSE OVER () (1))\nPRINT x\n"), "1\n");
}

#[test]
fn a_named_capture_reads_back_in_the_block() {
    assert_eq!(
        output("LET a = (7)\nLET x = (CLOSE OVER (a) (a))\nPRINT x\n"),
        "7\n"
    );
}

#[test]
fn a_block_local_binding_feeds_the_tail_and_escapes_nowhere() {
    assert_eq!(
        output("LET a = (7)\nLET x = (CLOSE OVER (a) ((LET b = (a)) (b)))\nPRINT x\n"),
        "7\n"
    );
    // `b` is the block's own binding: the enclosing scope never sees it, only the tail value.
    let error = error_of(
        "LET a = (7)\nLET x = (CLOSE OVER (a) ((LET b = (a)) (b)))\n",
        "b",
    );
    assert!(
        matches!(&error.kind, KErrorKind::UnboundName(name) if name == "b"),
        "a block-local binding must not escape the block, got {error}",
    );
}

/// A type-channel capture: the handle is lifetime-free and copies by value, so a type declared in
/// a producer frame is still nameable from the escaped block. Without the capture the same block
/// reports the name unbound — the block's chain reaches the eternal tier and stops there.
#[test]
fn a_type_capture_carries_a_per_call_type_into_the_block() {
    let producer = "LET mk = (FN :{n :Number} -> Any = (\
        (NEWTYPE Meters = Number)\
        (CLOSE OVER (%CAPTURES%) ((LET g = (FN :{} -> Any = (Meters n))) (g)))))\n\
        LET esc = (mk {n = 3})\n";
    assert_eq!(
        output(&format!(
            "{}PRINT (esc {{}})\n",
            producer.replace("%CAPTURES%", "Meters n")
        )),
        "Meters(3)\n"
    );
    let error = error_of(&producer.replace("%CAPTURES%", "n"), "esc {}");
    assert!(
        matches!(&error.kind, KErrorKind::UnboundName(name) if name == "Meters"),
        "an uncaptured per-call type must be unbound inside the block, got {error}",
    );
}

/// A capture naming nothing at all is this statement's own `UnboundName`, not a block-time failure.
#[test]
fn an_unresolvable_capture_is_unbound_at_the_form() {
    let error = error_of("", "CLOSE OVER (nope) (1)");
    assert!(
        matches!(&error.kind, KErrorKind::UnboundName(name) if name == "nope"),
        "expected UnboundName for an unresolvable capture, got {error}",
    );
}

// ---------- AC 1: a data-only capture set severs the producer chain ----------

/// **The severed closure's reach is flat in the producer's depth.** Held while `esc` is alive, the
/// run keeps the run root alone — however many per-call frames the closure was built inside, and
/// every one of those frames has already died. The block's own region does not survive either: the
/// escape seam consolidates the block scope into the destination, so what escapes borrows nothing
/// that was built to produce it.
// The census below reads the *mechanism*: `seam-force-pin` overrides the chooser and pins
// every callable escape, so only a build that lets the chooser decide can assert the count.
// The language-output assertions in this file are never gated.
#[cfg(not(feature = "seam-force-pin"))]
#[test]
fn a_severed_closure_pins_only_its_block_region() {
    for depth in [0usize, 1, 3, 5] {
        assert_eq!(
            held_and_released(&producer_chain(
                depth,
                &format!("CLOSE OVER (n) ({THUNK})"),
                ""
            )),
            (1, 0),
            "a severed closure over a {depth}-deep producer chain must pin nothing of its own",
        );
    }
}

/// The same program **without** the form, measured on the same counter. A plain closure over a
/// data-only environment is flat in producer depth too: the escape seam prices the crossing and
/// consolidates, rebuilding the captured chain at the destination, so no producer frame survives it
/// either. The verdict is a price, not a shape — an environment large against the region a pin
/// would retain stays pinned, which is what the chooser's own unit coverage pins down.
// The census below reads the *mechanism*: `seam-force-pin` overrides the chooser and pins
// every callable escape, so only a build that lets the chooser decide can assert the count.
// The language-output assertions in this file are never gated.
#[cfg(not(feature = "seam-force-pin"))]
#[test]
fn a_plain_closure_consolidates_at_the_escape_seam() {
    for depth in [0usize, 1, 3, 5] {
        assert_eq!(
            held_and_released(&producer_chain(depth, THUNK, "")).0,
            1,
            "a plain closure's data-only environment consolidates at every crossing",
        );
    }
}

/// Severance is not amnesia: the escaped closure still answers with the captured value after every
/// frame it was built in has died.
#[test]
fn a_severed_closure_still_answers_after_its_producers_die() {
    assert_eq!(
        output(&producer_chain(
            3,
            &format!("CLOSE OVER (n) ({THUNK})"),
            "PRINT (esc {})\n"
        )),
        "7\n"
    );
}

// ---------- AC 2: the data copy is transitive ----------

/// A capture whose borrows lead somewhere — a record of a list of strings and a nested record —
/// is rebuilt whole at the block's door. A shallow copy would leave the leaves borrowing the
/// producer's region and hold it up; the count says nothing is held, and rendering what escaped
/// says every leaf arrived.
#[test]
fn a_captured_structure_is_copied_transitively() {
    let build = "(LET deep = ({tags = [\"alpha\" \"omega\"] inner = {label = \"leaf\"}}))";
    let block = "CLOSE OVER (deep) ((LET g = (FN :{} -> Any = (deep))) (g))";
    #[cfg(not(feature = "seam-force-pin"))]
    for depth in [0usize, 1, 3] {
        assert_eq!(
            held_and_released(&producer_chain(depth, &format!("{build} ({block})"), "")),
            (1, 0),
            "a transitively copied capture leaves the producer chain free at depth {depth}",
        );
    }
    assert_eq!(
        output(&producer_chain(
            3,
            &format!("{build} ({block})"),
            "PRINT (esc {})\n"
        )),
        "{inner = {label = leaf}, tags = [alpha, omega]}\n"
    );
}

// ---------- AC 3: a pinned callable capture names exactly its home ----------

/// A chain whose **outermost** frame declares `HELPER` and whose innermost runs `block`. The
/// declaring frame's lexical parent is the run root, so pinning it pins nothing further; every
/// frame between it and the block is a pure intermediary.
fn declaring_chain(depth: usize, block: &str, trailer: &str) -> String {
    let mut body = block.to_string();
    for level in (1..=depth).rev() {
        body = format!("(LET f{level} = (FN :{{}} -> Any = ({body}))) (f{level} {{}})");
    }
    // At depth 0 the block is `f0`'s second statement and needs its own parentheses; deeper, `body`
    // is already the pair of parenthesized statements that enter the next level.
    let statements = if depth == 0 {
        format!("({body})")
    } else {
        body
    };
    format!(
        "LET f0 = (FN :{{n :Number}} -> Any = (\
             (FN (HELPER x :Number) -> Number = (x + n)) {statements}))\n\
         LET esc = (f0 {{n = 7}})\n{trailer}"
    )
}

/// The block a closed-over registration escapes in: a thunk that dispatches `HELPER`.
const HELPER_THUNK: &str = "CLOSE OVER () ((LET g = (FN :{} -> Number = (HELPER 1))) (g))";

/// A per-call `FN` registration lives in the frame that declared it, so closing over it pins that
/// one frame — and *only* it. Implicit close copies the registration in **pinned**, which is what
/// keeps that home alive; the block's own region does not survive beside it, because the escaping
/// thunk crosses the priced seam and its captured chain consolidates. The count is therefore the
/// run root and that single pinned home, flat however many further frames the block was nested
/// inside; the intermediaries all free.
///
/// Neither the absolute count nor exact flatness is asserted. The gauge is thread-local and carries
/// a ±1 that tracks which measurement mints the program region, and the crossing is **priced**, so a
/// deeper chain — whose regions are larger against the same small environment — can consolidate
/// where the shallow one pins. What the retention may never do is *grow* with the depth the block
/// was nested at, and that is the claim measured here.
#[test]
fn a_captured_registration_pins_its_home_frame_alone() {
    // The gauge is thread-local and the program region is minted once per thread, so whichever
    // measurement mints it reads one more than the rest — a ±1 that tracks test ordering, not the
    // program under test. One discarded measurement puts every count below in the same regime.
    let _warm = held_and_released(&declaring_chain(0, HELPER_THUNK, ""));
    let counts: Vec<(usize, usize)> = [0usize, 1, 3]
        .into_iter()
        .map(|depth| held_and_released(&declaring_chain(depth, HELPER_THUNK, "")))
        .collect();
    assert!(
        counts.windows(2).all(|pair| pair[0].0 >= pair[1].0),
        "the pinned set must not grow with the depth the block was nested at, got {counts:?}",
    );
    assert!(
        counts.iter().all(|(_, released)| *released == 0),
        "everything frees once the escaped value drops, got {counts:?}",
    );
}

/// Retention is transitive: the pinned registration's own captures — here the producer's parameter
/// `n`, which lives in the frame the registration pins — are still readable when the escaped
/// closure finally dispatches it, long after every call that built it returned.
#[test]
fn a_captured_registration_runs_after_its_producers_die() {
    assert_eq!(
        output(&declaring_chain(3, HELPER_THUNK, "PRINT (esc {})\n")),
        "8\n"
    );
}

/// The explicit half of the same rule: a signature-shaped pattern names one full bucket key and
/// captures the registration under it.
#[test]
fn a_pattern_capture_names_one_bucket_key() {
    assert_eq!(
        output(
            "FN (HELPER x :Number) -> Number = (x + 1)\n\
             LET x = (CLOSE OVER ((HELPER _)) (HELPER 1))\n\
             PRINT x\n"
        ),
        "2\n"
    );
}

/// A pattern capture reads its overloads out of the table that actually holds them: through a
/// `USING` window the seals live in the opened module's region, not the window scope's, and the
/// lift has to name that region as their home.
#[test]
fn a_pattern_capture_reaches_a_using_window_registration() {
    assert_eq!(
        output(
            "MODULE m = (FN (HELPER q :Number) -> Number = (q + 1))\n\
             LET x = (USING m SCOPE (CLOSE OVER ((HELPER _)) (HELPER 1)))\n\
             PRINT x\n"
        ),
        "2\n"
    );
}

/// A key names a registration *set*: every visible overload under it is captured, so the escaped
/// block still dispatches on argument type.
#[test]
fn a_pattern_capture_takes_every_overload_under_the_key() {
    assert_eq!(
        output(
            "FN (HELPER x :Number) -> Str = (\"number\")\n\
             FN (HELPER x :Str) -> Str = (\"string\")\n\
             LET x = (CLOSE OVER ((HELPER _)) (HELPER 1))\n\
             LET y = (CLOSE OVER ((HELPER _)) (HELPER \"a\"))\n\
             PRINT x\nPRINT y\n"
        ),
        "number\nstring\n"
    );
}

/// `CLOSE OVER ((HELPER _))` and `CLOSE OVER (a)` both reach the reader through a peeled group —
/// the parser hands back the pattern itself for the former and a one-part list for the latter — so
/// the two shapes are pinned side by side.
#[test]
fn a_single_capture_survives_the_redundant_group_peel() {
    assert_eq!(
        output(
            "FN (HELPER x :Number) -> Number = (x + 1)\n\
             LET a = (5)\n\
             LET x = (CLOSE OVER ((HELPER _)) (HELPER 1))\n\
             LET y = (CLOSE OVER (a) (a))\n\
             PRINT x\nPRINT y\n"
        ),
        "2\n5\n"
    );
}

// ---------- AC 4: an in-flight capture parks ----------

/// Every statement of a `run` is submitted before any of them steps, so a capture naming the
/// statement above it meets a *claim*, not a value. The form parks on the producer and completes
/// when it lands — an unparked read would have raised `UnboundName` instead.
#[test]
fn a_capture_of_an_in_flight_binding_parks_and_completes() {
    assert_eq!(
        output(
            "LET slow = (FN :{q :Number} -> Number = (q + 1))\n\
             LET a = (slow {q = 6})\n\
             LET x = (CLOSE OVER (a) (a))\n\
             PRINT x\n"
        ),
        "7\n"
    );
}

/// The same wait on the dispatch channel: a pattern capture of a registration that is still
/// finalizing parks on its bucket claim.
#[test]
fn a_pattern_capture_of_an_in_flight_registration_parks_and_completes() {
    assert_eq!(
        output(
            "LET base = (10)\n\
             FN (HELPER x :Number) -> Number = (x + base)\n\
             LET x = (CLOSE OVER ((HELPER _)) (HELPER 1))\n\
             PRINT x\n"
        ),
        "11\n"
    );
}

/// Implicit close waits too: the block closes over a registration submitted in the same batch, so
/// what it captures never depends on the drain order.
#[test]
fn implicit_close_parks_on_an_in_flight_registration() {
    assert_eq!(
        output(
            "LET mk = (FN :{n :Number} -> Any = (\
                 (FN (HELPER x :Number) -> Number = (x + n))\
                 (CLOSE OVER () ((LET g = (FN :{} -> Number = (HELPER 1))) (g)))))\n\
             LET esc = (mk {n = 10})\n\
             PRINT (esc {})\n"
        ),
        "11\n"
    );
}

// ---------- AC 5: capture-list tokens ----------

/// A bare keyword names no registration: a bucket key is the *whole* call shape, and `HELPER`
/// alone is not the key `(HELPER _)` is registered under.
#[test]
fn a_bare_keyword_names_no_registration() {
    let error = error_of(
        "FN (HELPER x :Number) -> Number = (x + 1)\n",
        "CLOSE OVER (HELPER) (1)",
    );
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(detail)
            if detail.contains("names no registration")),
        "a bare keyword capture must be a shape error, got {error}",
    );
}

/// A keyword sitting among names is not a list of captures at all — the reader takes the slot as
/// one pattern and refuses the name inside it.
#[test]
fn a_keyword_mixed_into_a_capture_list_is_refused() {
    let error = error_of(
        "LET a = (1)\nFN (HELPER x :Number) -> Number = (x + 1)\n",
        "CLOSE OVER (a HELPER) (1)",
    );
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(detail)
            if detail.contains("keywords and `_` holes only")),
        "a mixed capture list must be a shape error, got {error}",
    );
}

/// An all-hole pattern names every bucket and so names none.
#[test]
fn an_all_hole_pattern_names_no_bucket() {
    let error = error_of("", "CLOSE OVER ((_ _)) (1)");
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(detail)
            if detail.contains("needs at least one keyword")),
        "an all-hole pattern must be a shape error, got {error}",
    );
}

/// A pattern naming a key nothing is registered under is refused at the form, not silently dropped.
#[test]
fn a_pattern_for_an_absent_registration_is_refused() {
    let error = error_of("", "CLOSE OVER ((NOWHERE _)) (1)");
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(detail)
            if detail.contains("names no registration")),
        "an unregistered pattern must be a shape error, got {error}",
    );
}

/// A literal is neither a name nor a pattern.
#[test]
fn a_literal_capture_is_refused() {
    let error = error_of("", "CLOSE OVER (1) (1)");
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(_)),
        "a literal capture must be a shape error, got {error}",
    );
}

// ---------- AC 6: what an escaped block body can still reach ----------

/// A per-call **operator** registration is a dispatch registration for closing purposes: the
/// escaped body still reduces a three-operand run through it.
#[test]
fn an_escaped_body_applies_a_per_call_operator() {
    assert_eq!(
        output(
            "LET mk = (FN :{n :Number} -> Any = (\
                 (OP #(⊕) OVER Number = (left + right))\
                 (CLOSE OVER () ((LET g = (FN :{} -> Number = (1 ⊕ 2 ⊕ 3))) (g)))))\n\
             LET esc = (mk {n = 1})\n\
             PRINT (esc {})\n"
        ),
        "6\n"
    );
}

/// Two scopes on the walked chain may declare one operator probe differently — a bare `OP` and a
/// `GROUP` whose member set is wider — and the inner one shadows the outer whole, since operator
/// resolution stops at the first scope holding the probe. Implicit close keeps that innermost
/// declaration and drops the outer one: flattening both into the block's single registry would hit
/// the one-chaining-mode-per-scope rule, which shadowing scopes were never subject to, and turn a
/// run that reduces outside the block into an error inside it.
#[test]
fn a_shadowed_operator_declaration_does_not_conflict_when_flattened() {
    let source = |body: &str| {
        format!(
            "GROUP ops FOLD LEFT = (\
                 (OP #(⊕) OVER Number = (left + right))\
                 (OP #(⊖) OVER Number = (left))\
                 (LET seeded = (1 ⊕ 2)))\n\
             LET mk = (FN :{{n :Number}} -> Any = (\
                 (OP #(⊕) OVER Str = (left))\
                 (USING ops SCOPE {body})))\n\
             LET r = (mk {{n = 1}})\n\
             PRINT r\n"
        )
    };
    let bare = output(&source("(1 ⊕ 2 ⊕ 3)"));
    assert_eq!(
        bare, "6\n",
        "the run reduces through the window's group without the block"
    );
    assert_eq!(
        output(&source("(CLOSE OVER () (1 ⊕ 2 ⊕ 3))")),
        bare,
        "wrapping the same run in the block must not change what the probe resolves to",
    );
}

/// A per-call **module** binding is copied in pinned, so the escaped body still reads its members.
#[test]
fn an_escaped_body_reads_a_per_call_module() {
    assert_eq!(
        output(
            "LET mk = (FN :{n :Number} -> Any = (\
                 (MODULE inner = (LET v = 5))\
                 (CLOSE OVER () ((LET g = (FN :{} -> Number = (inner.v))) (g)))))\n\
             LET esc = (mk {n = 1})\n\
             PRINT (esc {})\n"
        ),
        "5\n"
    );
}

/// A `USING … SCOPE` window is part of the walked chain. Its surfaced *registrations* and
/// *modules* close implicitly — pinning the opened module's region — while a surfaced data member
/// is an ordinary capture the list names.
#[test]
fn an_escaped_body_reaches_through_a_using_window() {
    let shapes: &[(&str, &str, &str)] = &[
        (
            "a surfaced registration",
            "MODULE inner = (FN (HELPER q :Number) -> Number = (q + 1))",
            "CLOSE OVER () ((LET g = (FN :{} -> Number = (HELPER 1))) (g))",
        ),
        (
            "a surfaced module",
            "MODULE inner = (MODULE sub = (LET x = 2))",
            "CLOSE OVER () ((LET g = (FN :{} -> Number = (sub.x))) (g))",
        ),
        (
            "a surfaced value, named explicitly",
            "MODULE inner = (LET v = 2)",
            "CLOSE OVER (v) ((LET g = (FN :{} -> Number = (v))) (g))",
        ),
    ];
    for (name, module, block) in shapes {
        let source = format!(
            "LET mk = (FN :{{n :Number}} -> Any = (({module}) (USING inner SCOPE ({block}))))\n\
             LET esc = (mk {{n = 1}})\n\
             PRINT (esc {{}})\n"
        );
        assert_eq!(output(&source), "2\n", "{name} must survive the escape");
    }
}

// ---------- EVAL at the block boundary ----------

/// `EVAL` inside the block resolves against the block scope and its captures.
#[test]
fn eval_resolves_a_capture_inside_the_block() {
    assert_eq!(
        output("LET a = (7)\nLET x = (CLOSE OVER (a) ($(#(a))))\nPRINT x\n"),
        "7\n"
    );
}

/// A name the block did not capture is unbound at the boundary — the walk ends at the eternal
/// scope, so a per-call producer's binding is not reachable by spelling it dynamically.
#[test]
fn eval_of_an_uncaptured_producer_name_is_unbound() {
    let error = error_of(
        "LET mk = (FN :{n :Number} -> Any = (\
             (LET secret = (n + 1))\
             (CLOSE OVER () ((LET g = (FN :{} -> Number = ($(#(secret))))) (g)))))\n\
         LET esc = (mk {n = 1})\n",
        "esc {}",
    );
    assert!(
        matches!(&error.kind, KErrorKind::UnboundName(name) if name == "secret"),
        "an uncaptured producer name must be unbound inside the block, got {error}",
    );
}

/// The eternal chain stays visible through the block's lexical outer: the escaped body dispatches a
/// top-level definition and applies a builtin operator, neither of which is a capture.
#[test]
fn the_eternal_chain_stays_visible_inside_the_block() {
    assert_eq!(
        output(
            "FN (TOPLEVEL x :Number) -> Number = (x * 2)\n\
             LET mk = (FN :{n :Number} -> Any = (\
                 (CLOSE OVER () ((LET g = (FN :{} -> Number = ((TOPLEVEL 4) + 1))) (g)))))\n\
             LET esc = (mk {n = 1})\n\
             PRINT (esc {})\n"
        ),
        "9\n"
    );
}

/// The tail is a value like any other: a block whose tail is data returns data, and the whole
/// block region goes with the statement.
// Pins the copy verb's release of the block region; the `seam-force-pin` build pins the record's
// producer instead, keeping that one region for the run. The equivalence battery proves the choice
// is invisible to language output separately.
#[cfg(not(feature = "seam-force-pin"))]
#[test]
fn a_data_tail_leaves_no_region_behind() {
    assert_eq!(
        held_and_released(
            "LET mk = (FN :{n :Number} -> Any = (CLOSE OVER (n) ({a = n})))\nLET esc = (mk {n = 7})\n"
        ),
        (1, 0),
        "a copied-out data tail pins neither the block region nor the producer's",
    );
}

/// The escaped value is a callable, not a rendered result — the read-back path the liveness tests
/// measure against.
#[test]
fn the_escaped_tail_is_a_callable_value() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(&producer_chain(2, &format!("CLOSE OVER (n) ({THUNK})"), ""));
    let escaped = test_run.run_one(test_run.parse_one("esc"));
    assert!(
        matches!(escaped, KObject::KFunction(..)),
        "the block's tail escapes as the closure it built",
    );
}

// ---------- AC: the block's working copies die with the cart ----------

/// Run-root bytes reserved across `EVALS` fresh top-level evaluations of `form`, after a warmup
/// window that climbs the bump's chunk ladder.
///
/// `allocated_total` weighs both halves of the region — the typed sub-arena and the bump the
/// `Drop`-free families live in — and the bump half reports *reserved* chunk capacity, which
/// arrives in chunk-sized steps that double as the bump grows. So the shape is the one
/// `repeated_user_fn_calls_do_not_grow_run_root_per_call` (`fn_def::tests::arena`) establishes: the
/// warmup climbs the ladder before the baseline is taken, and the measured window is long enough
/// that per-evaluation growth would dominate a single step.
fn run_root_growth(form: &str) -> u64 {
    const WARMUP: usize = 200;
    const EVALS: usize = 4000;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    for _ in 0..WARMUP {
        let _ = test_run.run_one(test_run.parse_one(form));
    }
    let baseline = region.region().allocated_total();
    for _ in 0..EVALS {
        let _ = test_run.run_one(test_run.parse_one(form));
    }
    region.region().allocated_total() - baseline
}

/// Reserved capacity arrives in doubling chunk steps, so two windows measuring the same per-
/// evaluation cost can still differ by a whole step. Every bound comparing one window against
/// another admits that much before it reads a difference as a leak.
const CHUNK_SLACK: u64 = 512 * 1024;

/// The criterion's direct form. A `CLOSE OVER` evaluated over and over leaves the run-root arena
/// flat: its block's statements are frozen into the cart the form installs, so each evaluation's
/// copies are released with that cart instead of accumulating in the run root.
///
/// Two bounds, because "flat" has two readings and the block's statements have to be absent from
/// both. Against a **plain statement** run the same number of times: whatever the harness spends
/// per top-level dispatch, the form spends no more, so the block's own copies are not landing
/// beside it. And across **block size**: a six-statement block costs no more than a two-statement
/// one, which is the reading that fails outright if the statements are frozen into the run root —
/// there the difference would be four working copies per evaluation.
#[test]
fn repeated_close_over_leaves_run_root_flat() {
    let plain = run_root_growth("1");
    let two = run_root_growth("CLOSE OVER () ((LET b = (1)) (b))");
    let six = run_root_growth(
        "CLOSE OVER () ((LET b = (1)) (LET c = (1)) (LET d = (1)) (LET e = (1)) (LET f = (1)) (b))",
    );

    let budget = 3 * plain + CHUNK_SLACK;
    assert!(
        two < budget,
        "a repeated `CLOSE OVER` grew the run root by {two} bytes against a plain statement's \
         {plain} (expected < {budget}) — the block is leaving residue behind",
    );
    let budget = two + CHUNK_SLACK;
    assert!(
        six < budget,
        "run-root growth scaled with block size: {six} bytes for six statements against {two} for \
         two (expected < {budget}) — the statements are being frozen into the run root",
    );
}

/// A body that is not a statement block takes the no-split freeze — the whole expression is the
/// tail and the leading run is empty — and it must land in the cart just the same.
#[test]
fn single_expression_close_over_leaves_run_root_flat() {
    let plain = run_root_growth("1");
    let single = run_root_growth("CLOSE OVER () (1)");
    let budget = 3 * plain + CHUNK_SLACK;
    assert!(
        single < budget,
        "a repeated single-expression `CLOSE OVER` grew the run root by {single} bytes against a \
         plain statement's {plain} (expected < {budget})",
    );
}

/// Carried through a recursion, the form is re-entered once per level while every level's frame is
/// still live — the shape that made a run-root-homed freeze unbounded. Measured as a *depth*
/// differential: the same countdown at depth 4 and depth 40, each run the same number of times. A
/// per-level leak would cost the deep window roughly ten times the shallow one; a freeze homed in
/// the cart costs the two the same, because the copies die with each level's block.
///
/// The countdown is the `Nat` (`Zero | Succ Nat`) unwind `fn_def::tests::tail_region_turnover`
/// uses, with the form as a leading statement of the body so the recursion carries it.
#[test]
fn recursion_carried_close_over_stays_flat_in_depth() {
    fn countdown_growth(depth: usize) -> u64 {
        const WARMUP: usize = 20;
        const RUNS: usize = 200;
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let mut source = String::from(
            "UNION Nat = (Zero :Null Succ :Nat)\n\
             FN (COUNTDOWN n :Nat) -> Str = (\
                 (CLOSE OVER () ((LET b = (1)) (b)))\
                 (MATCH (n) OVER Nat -> :Str WITH (\
                     Zero -> (\"done\")\
                     Succ -> (COUNTDOWN it)\
                 ))\
             )\n\
             LET n0 = (Nat.Zero null)\n",
        );
        for level in 1..=depth {
            source.push_str(&format!("LET n{level} = (Nat.Succ n{})\n", level - 1));
        }
        test_run.run(&source);
        let call = format!("COUNTDOWN n{depth}");
        for _ in 0..WARMUP {
            let _ = test_run.run_one(test_run.parse_one(&call));
        }
        let baseline = region.region().allocated_total();
        for _ in 0..RUNS {
            let _ = test_run.run_one(test_run.parse_one(&call));
        }
        region.region().allocated_total() - baseline
    }

    let shallow = countdown_growth(4);
    let deep = countdown_growth(40);
    let budget = 2 * shallow + CHUNK_SLACK;
    assert!(
        deep < budget,
        "run-root growth scaled with recursion depth: {deep} bytes at depth 40 against {shallow} \
         at depth 4 (expected < {budget}) — a carried `CLOSE OVER` is leaving residue per level",
    );
}
