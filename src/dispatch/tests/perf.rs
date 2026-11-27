//! The `perf` report: for each runnable tutorial snippet, the `NameQuestion`s its binding graph's
//! instances asked and the stored questions they evaluated, at the load and across the run.
//!
//! A snippet is a `koan` block followed directly by a `text` block, as `tools/verify_snippets.py`
//! reads them. The counts are the program's own graph's; a composition's graph counts apart.
//!
//! ```sh
//! cargo test --features perf perf_report -- --nocapture
//! ```

use bindgraph::Counts;

use super::{Koan, output};
use crate::build::lay::BaseInstances;
use crate::program::CellSubstrate;
use crate::values::BuildBudget;

/// Expression shapes the interpreter does not run yet, as the snippet checker skips them.
const PENDING: [&str; 2] = ["TYPE", "CLOSE"];

/// Each runnable snippet in `markdown`: the code of a `koan` block followed by a `text` block with
/// only whitespace between them.
fn snippets(markdown: &str) -> Vec<&str> {
    // Each fenced block: its tag, its body, and where it starts and ends in `markdown`.
    let mut blocks = Vec::new();
    let mut from = 0;
    while let Some(open) = markdown[from..].find("```").map(|at| from + at) {
        let tag_end = open + 3 + markdown[open + 3..].find('\n').unwrap_or(0);
        let Some(close) = markdown[tag_end..].find("\n```").map(|at| tag_end + at) else {
            break;
        };
        let body = &markdown[(tag_end + 1).min(close)..close];
        blocks.push((&markdown[open + 3..tag_end], body, open, close + 4));
        from = close + 4;
    }
    let runnable = |pair: &[(&str, &str, usize, usize)]| {
        let [(tag, _, _, end), (next, _, start, _)] = pair else {
            return false;
        };
        *tag == "koan" && *next == "text" && markdown[*end..*start].trim().is_empty()
    };
    let pending = |code: &str| {
        (code.split(|c: char| !c.is_ascii_alphanumeric() && c != '_'))
            .any(|word| PENDING.contains(&word))
    };
    (blocks.windows(2))
        .filter(|pair| runnable(pair) && !pending(pair[0].1))
        .map(|pair| pair[0].1)
        .collect()
}

/// What loading and running `source` counted: at the load, and across the run, or `None` where
/// the load refuses it.
fn counted(source: &str) -> Option<(Counts, Counts)> {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<input>", 8, BuildBudget::default(), output()).ok()?;
    Some(substrate.with(|running| {
        let _ = running.ran();
        (running.binding_graph().counts, running.instances().counts())
    }))
}

#[test]
fn perf_report() {
    let tutorial = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tutorial");
    let mut chapters: Vec<_> = (std::fs::read_dir(&tutorial).expect("the tutorial is readable"))
        .map(|entry| entry.expect("a tutorial entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    chapters.sort();
    let (mut load, mut run, mut refused) = (Counts::default(), Counts::default(), 0);
    println!("snippet                      load asks  load runs  run asks  run runs");
    for chapter in &chapters {
        let markdown = std::fs::read_to_string(chapter).expect("a chapter is readable");
        let name = chapter
            .file_stem()
            .expect("a chapter has a name")
            .to_string_lossy();
        for (at, code) in snippets(&markdown).iter().enumerate() {
            let label = format!("{name}#{}", at + 1);
            match counted(&format!("{code}\n")) {
                Some((at_load, at_run)) => {
                    println!(
                        "{label:<28} {:>9}  {:>9}  {:>8}  {:>8}",
                        at_load.asks, at_load.runs, at_run.asks, at_run.runs
                    );
                    (load.asks, load.runs) = (load.asks + at_load.asks, load.runs + at_load.runs);
                    (run.asks, run.runs) = (run.asks + at_run.asks, run.runs + at_run.runs);
                }
                None => {
                    println!("{label:<28} refused at the load");
                    refused += 1;
                }
            }
        }
    }
    println!(
        "{:<28} {:>9}  {:>9}  {:>8}  {:>8}",
        "total", load.asks, load.runs, run.asks, run.runs
    );
    println!("{refused} snippets refused at the load");
}
