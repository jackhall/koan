//! Shared test scaffolding for `memory`'s suites, and the pin on program storage's one store.

use crate::tests::boundary::{contains_word, strip_comments_and_strings};

use super::substrate::{CellGraph, ReleaseAbsorption, Verdict, Writer, reattachable};

/// A continuation family for a graph whose cells only store.
struct Step;
reattachable!(Step => ());

/// Run `step` inside one cell of a one-slot graph, handing it the cell's writer.
pub(crate) fn in_cell<R>(step: impl for<'cell> FnOnce(Writer<'cell>) -> R) -> R {
    let mut graph: CellGraph<'static, Step> = CellGraph::new(1, |_| Verdict::Pin);
    let cell = graph
        .create(None)
        .expect("a one-slot graph has a free slot");
    let out = graph
        .enter(cell, |context| step(context.writer()))
        .expect("a fresh cell is enterable");
    graph
        .release(cell, ReleaseAbsorption::IntoHolder)
        .expect("a cell outside its step releases");
    out
}

/// Program storage holds one store and hands out one capability: a `Writer`. The compiler cannot
/// say "no method returns a bump allocator", so this reads the module's own source — a second
/// store, or a door widening back onto the bump tier, would have to name a bump-tier type here.
#[test]
fn program_storage_names_no_bump_tier_type() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/memory/program.rs"
    ))
    .expect("program storage's source reads");
    let code = strip_comments_and_strings(&source);
    for word in ["Bump", "BumpAllocator", "BumpVec", "bumpalo"] {
        assert!(
            !contains_word(&code, word),
            "program storage names {word}: it holds one `cellgraph` store, written through a `Writer`",
        );
    }
}
