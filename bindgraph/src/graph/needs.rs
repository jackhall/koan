//! Where a knot can form: the text's arcs of need and their strongly connected components.
//!
//! Each need a demand can read is an arc between two vertices of the text, whatever the view:
//! its answered names and its questions. An evaluation waits only on the answered names its own
//! node binds, and on a callee name only where the callee's entry binds it, so the arcs run
//! from a question to each answered name it reads that its owner binds; from an answered name
//! to its deciding question; from a name typed on an arm to each answered name its deciding
//! question reads that the name's node binds; from a name typed on a site to each answered name
//! its term mentions that the name's node binds; and from a name typed through a call to each
//! answered name the call passes that the name's node binds, and to the callee name it takes
//! where the callee's entry binds it.
//!
//! The arcs come from every arm the text lays, so every knot under every binding lies in one
//! component that holds a cycle: a **text component**. The evaluation keeps a demand outside
//! every text component out of Tarjan's books ([evaluate.rs](../bound/evaluate.rs)). A text
//! component bounds where a knot can form, never what it is
//! ([the crate's README](../../README.md#instances)). Components are [`grows`]'s Kosaraju.

use super::{Graph, Source, grows};
use crate::domain::Facts;
use crate::id::{ComponentId, NameId, NodeId, QuestionId};

/// Each name's and each question's text component.
pub(super) struct TextComponents {
    pub(super) names: Vec<Option<ComponentId>>,
    pub(super) questions: Vec<Option<ComponentId>>,
}

/// The text components of `graph`'s arcs of need, numbered in order of each one's first vertex:
/// names in `NameId` order, then questions in `QuestionId` order.
pub(super) fn text_components<F: Facts>(graph: &Graph<F>) -> TextComponents {
    // Vertices: each name at its index, then each question after every name.
    let names = graph.names.len();
    let vertices = names + graph.questions.len();
    let question = |question: QuestionId| names + question.index();
    let waited = |at: NodeId, reads: &[NameId]| -> Vec<usize> {
        let bound_there = reads
            .iter()
            .filter(|&&read| graph.answered_at(read) == Some(at));
        bound_there.map(|read| read.index()).collect()
    };
    let mut forward: Vec<Vec<usize>> = vec![Vec::new(); vertices];
    for (index, placed) in graph.answered.iter().enumerate() {
        let Some(placed) = placed else { continue };
        let arcs = &mut forward[index];
        arcs.extend(placed.deciding.map(question));
        for &edge in &placed.edges {
            match *graph.answers_edge(edge).source() {
                Source::Arm { question, .. } => {
                    arcs.extend(waited(placed.node, graph.question(question).reads()));
                }
                Source::Site { .. } => {
                    arcs.extend(waited(placed.node, graph.answers_edge(edge).mentions()));
                }
                Source::Call { edge: call, takes } => {
                    arcs.extend(waited(placed.node, graph.edge(call).passes()));
                    let entry = graph.entry(graph.edge(call).callee());
                    if graph.answered_at(takes) == Some(entry) {
                        arcs.push(takes.index());
                    }
                }
            }
        }
    }
    for (index, asked) in graph.questions.iter().enumerate() {
        forward[names + index] = waited(asked.owner(), asked.reads());
    }

    let component = grows::components(&forward);
    // A component holds a cycle where it has two members, or one with an arc to itself.
    let mut members = vec![0usize; vertices];
    let mut looped = vec![false; vertices];
    for (vertex, arcs) in forward.iter().enumerate() {
        members[component[vertex]] += 1;
        looped[component[vertex]] |= arcs.contains(&vertex);
    }
    // Number the text components densely, by first vertex.
    let mut ids: Vec<Option<ComponentId>> = vec![None; vertices];
    let mut count = 0;
    let mut of: Vec<Option<ComponentId>> = (0..vertices)
        .map(|vertex| {
            let at = component[vertex];
            let cycles = members[at] > 1 || looped[at];
            cycles.then(|| {
                *ids[at].get_or_insert_with(|| {
                    count += 1;
                    ComponentId::new(count - 1)
                })
            })
        })
        .collect();
    let questions = of.split_off(names);
    TextComponents {
        names: of,
        questions,
    }
}
