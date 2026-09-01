//! **Lazy close** acceptance — the copy verb going *through* callable leaves. A closure crossing a
//! priced escape seam, or captured by an explicit `CLOSE OVER`, has the per-call portion of its
//! captured scope chain rebuilt at the destination, so what escapes borrows nothing that was built
//! to produce it.
//!
//! Stated the way the rest of this suite states severance: through the memory substrate. The
//! discriminator is `held_and_released` — how many regions survive while the escaped value is still
//! held — paired with what the escaped value answers once every frame it was built in has died. A
//! count alone could be a value that lost its environment; an answer alone could be a value keeping
//! the whole producer chain alive. Together they are the claim.
//!
//! The region harness lives beside `CLOSE OVER`'s own tests because the form is one of the two
//! surfaces the copy recurses at; the escape seam is the other, and the cases below exercise both.
//!
//! One claim is **not** measured here: that a definition site pays nothing for the pricing. That is
//! a cost statement, and its instrument is `tests/allocation_baseline.rs` — the declaration shape's
//! per-name term, which the consolidation left at parity.

use super::{held_and_released, output};

/// **A recursive closure copies to a closure whose captured scope binds the copy itself.** The
/// producer frame registers two `STEP` overloads in one dispatch bucket, each capturing that same
/// frame scope, and the first calls the second by name — the `scope → function → scope` cycle. The
/// copy terminates on it (the memo holds the scope before its tables are filled) and the escaped
/// thunk still dispatches through the bucket after its producer has died.
#[test]
fn a_self_referential_dispatch_bucket_copies_to_one_copied_scope() {
    let source = "LET mk = (FN :{n :Number} -> Any = (\
         (FN (STEP s :Str) -> Number = (n))\
         (FN (STEP k :Number) -> Number = (STEP \"x\"))\
         (FN :{} -> Number = (STEP 1))))\n\
         LET esc = (mk {n = 9})\n";
    assert_eq!(
        output(&format!("{source}PRINT (esc {{}})\n")),
        "9\n",
        "the copied bucket dispatches after the producer frame has died",
    );
    assert_eq!(
        held_and_released(source),
        (1, 0),
        "a self-referential environment leaves nothing of its producer alive",
    );
}

/// **Two closures over one defining scope copy to two closures over one copied scope.** `a` and `b`
/// are bound in the same producer frame and both capture it; the escaping thunk calls both. Sharing
/// is what the sum proves: two copies of the frame would still answer, but the memo is what makes
/// them one, and the flat count is what says neither kept the source alive.
#[test]
fn sibling_closures_share_one_copied_scope() {
    let source = "LET mk = (FN :{n :Number} -> Any = (\
         (LET a = (FN :{} -> Number = (n + 1)))\
         (LET b = (FN :{} -> Number = (n + 2)))\
         (FN :{} -> Number = ((a {}) + (b {})))))\n\
         LET esc = (mk {n = 10})\n";
    assert_eq!(output(&format!("{source}PRINT (esc {{}})\n")), "23\n");
    assert_eq!(
        held_and_released(source),
        (1, 0),
        "two closures over one scope leave one producer chain free",
    );
}

/// **A `CLOSE OVER` callable capture severs transitively: its data is copied and its own callable
/// references recurse.** `helper` is produced by a frame that has already closed and then captured
/// by name; `helper` itself captures a scope binding `inner`, so severing it means rebuilding a
/// chain two callables deep. The block's thunk escapes and answers with nothing of either producer
/// left alive.
#[test]
fn a_close_over_callable_capture_severs_transitively() {
    let source = "LET make = (FN :{n :Number} -> Any = (\
         (LET inner = (FN :{} -> Number = (n * 3)))\
         (FN :{} -> Number = (inner {}))))\n\
         LET helper = (make {n = 5})\n\
         LET mk = (FN :{h :Any} -> Any = (\
         CLOSE OVER (h) ((LET g = (FN :{} -> Number = (h {}))) (g))))\n\
         LET esc = (mk {h = helper})\n";
    assert_eq!(output(&format!("{source}PRINT (esc {{}})\n")), "15\n");
    assert_eq!(
        held_and_released(source),
        (1, 0),
        "a transitively severed callable capture leaves both producer chains free",
    );
}

/// **An environment the copy cannot rebuild is pinned, and the value still answers.** A `CLOSE OVER`
/// capture whose source scope is the very scope the statement sits in meets that scope still
/// **open** — a further bind is legal there, so the copy would be free to miss one. It downgrades
/// to a pin rather than waiting: the escaped closure answers, and the regions it kept are the price
/// of that. Nothing parks, and nothing can: no edge of any kind is added anywhere on this path.
#[test]
fn an_unready_environment_pins_and_still_answers() {
    let source = "LET mk = (FN :{n :Number} -> Any = (\
         (LET helper = (FN :{} -> Number = (n * 3)))\
         (CLOSE OVER (helper) ((LET g = (FN :{} -> Number = (helper {}))) (g)))))\n\
         LET esc = (mk {n = 5})\n";
    assert_eq!(
        output(&format!("{source}PRINT (esc {{}})\n")),
        "15\n",
        "a pinned environment answers exactly as a copied one does",
    );
    let (held, released) = held_and_released(source);
    assert!(
        held > 1,
        "an open captured scope must pin rather than consolidate, got {held} regions held",
    );
    assert_eq!(released, 0, "the run frees everything once the value drops");
}
