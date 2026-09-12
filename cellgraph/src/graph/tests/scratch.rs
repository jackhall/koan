//! The graph's scratch region, as the verbs use it: reset at every verb's entry and never inside
//! one, and warm enough from construction that a verb's transients ask the allocator for nothing.
//!
//! These are the lib-test mirror of the harness's allocation criteria. The harness meters the
//! global allocator per verb; these read the region's own two figures — the chunk bytes it holds,
//! and the bytes handed out of them — directly.

use super::super::*;
use super::{Number, Owned, number_here, one, operand, pin, pinned};

/// Operands in the placement the dirtying helper makes. Wide enough that the per-operand lists
/// outgrow the region's first chunk, which is what leaves a reading behind: a bump rewinds the
/// last allocation when it is freed, so only a list that *grew* strands the buffer it came from.
const WIDE: usize = 256;

/// A placement of one value over `operands` copies of a source carrier, into another cell — the
/// widest transient a step builds, since the crossed-operand list and the views are both sized by
/// the operand count.
fn place_over(graph: &mut CellGraph<Owned>, from: SlabHandle, into: SlabHandle, operands: usize) {
    graph
        .enter(from, |context| {
            let source = number_here(context, 1);
            let carriers: Vec<_> = (0..operands).map(|_| operand(&source)).collect();
            context
                .alloc_into::<Number, Number>(into, &carriers, |writer, views| {
                    one(writer, views.iter().map(|view| *pinned(view)).sum())
                })
                .unwrap();
        })
        .unwrap();
}

/// A graph whose region still holds the last verb's transients, and the two cells it was built
/// with. The assertion is the placement half of the criterion: a step's crossed operands and the
/// views its build closure received are **in the region**, so a wide enough placement is readable
/// in the region's own occupancy after the step returns.
fn dirtied() -> (CellGraph<Owned>, SlabHandle, SlabHandle) {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let producer = graph.create(None, None).unwrap();
    let consumer = graph.create(None, None).unwrap();
    place_over(&mut graph, producer, consumer, WIDE);
    assert!(
        graph.scratch_at_rest().in_use() > 0,
        "a placement over {WIDE} operands left nothing in the region"
    );
    (graph, producer, consumer)
}

/// Run `verb` against a region the verb before it left occupied, and check that the verb reset it.
///
/// The reading falling is the whole proof, and needs no figure to compare against: a bump hands
/// bytes back only to the transient that took them last, so a verb's occupancy at its exit is never
/// below its occupancy at its entry. Lower than what the verb inherited therefore means the entry
/// cleared it — and a verb that builds transients of its own is held to the same statement as one
/// that builds none, since its own bytes are counted on the low side.
fn resets_at_its_entry(graph: &mut CellGraph<Owned>, verb: impl FnOnce(&mut CellGraph<Owned>)) {
    let inherited = graph.scratch_at_rest().in_use();
    verb(graph);
    assert!(
        graph.scratch_at_rest().in_use() < inherited,
        "a verb carried its predecessor's {inherited} occupied bytes past its entry"
    );
}

/// The reset sits at each verb's entry as its own statement, so each of the three is checked
/// against a region the verb before it left occupied.
#[test]
fn a_create_clears_the_region_at_its_entry() {
    let (mut graph, _, _) = dirtied();
    resets_at_its_entry(&mut graph, |graph| {
        graph.create(None, None).unwrap();
    });
}

#[test]
fn an_enter_clears_the_region_at_its_entry() {
    let (mut graph, producer, _) = dirtied();
    resets_at_its_entry(&mut graph, |graph| {
        graph.enter(producer, |_| ()).unwrap();
    });
}

#[test]
fn a_release_clears_the_region_at_its_entry() {
    let (mut graph, producer, consumer) = dirtied();
    // The consumer holds the producer's storage, so the release seals rather than reclaims and the
    // whole cascade runs — its own transients included.
    graph
        .enter(consumer, |context| context.hold(producer).unwrap())
        .unwrap();
    // That step reset the region on the way in, so the release would inherit an empty one. Dirty it
    // again, which is also the state a release meets in a run that is doing anything.
    place_over(&mut graph, producer, consumer, WIDE);
    resets_at_its_entry(&mut graph, |graph| {
        graph.release(producer, ReleaseAbsorption::Refused).unwrap();
    });
}

/// A graph warm from construction grows no chunk: the first round's transients fit the chunk the
/// constructor sized, and every round after it reuses the same bytes. This is the lib-test mirror
/// of the harness's "no verb's allocation count grows with the operand count" criterion — the
/// bytes a round asks for come out of a chunk that was paid at construction.
///
/// The round covers both verbs that build transients — a placement under `enter`, and a `release`
/// whose cascade seals a held producer — so the figure spans the placement's view list and the
/// disposal's nested worklists alike.
#[test]
fn a_warm_scratch_grows_no_chunk_across_repeated_verbs() {
    let mut graph: CellGraph<Owned> = CellGraph::new(8, pin);
    let consumer = graph.create(None, None).unwrap();

    let round = |graph: &mut CellGraph<Owned>| {
        let producer = graph.create(None, None).unwrap();
        place_over(graph, producer, consumer, 8);
        graph
            .enter(consumer, |context| context.hold(producer).unwrap())
            .unwrap();
        graph.release(producer, ReleaseAbsorption::Refused).unwrap();
    };

    round(&mut graph);
    let warm = graph.scratch_at_rest().capacity();
    round(&mut graph);
    assert_eq!(
        graph.scratch_at_rest().capacity(),
        warm,
        "a second round of the same verbs grew the region"
    );
    assert_eq!(
        warm,
        Scratch::new().capacity(),
        "a round outgrew the chunk the constructor sized"
    );
}

/// A step that panics still hands the region back, so the graph it unwinds out of is usable and
/// the next verb finds a region rather than the `None` the take left behind.
///
/// This is where the hand-back is observable. A `release` parks the region under the same kind of
/// guard, but its cascade runs no embedder code, so the only panics reachable there are broken
/// internal invariants — which a test cannot stage without pretending one is broken.
#[test]
fn a_panicking_step_hands_the_region_back() {
    let (mut graph, producer, _) = dirtied();
    let gave_up = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        graph.enter(producer, |_| panic!("the step gives up")).ok();
    }));
    assert!(
        gave_up.is_err(),
        "the step's panic did not reach the caller"
    );
    // Back on the graph, and cleared by the entry of the step that then failed.
    assert_eq!(graph.scratch_at_rest().in_use(), 0);
    graph.create(None, None).unwrap();
}
