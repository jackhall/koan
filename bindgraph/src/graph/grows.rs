//! Refusing a recursion that grows: the name-level graph of mentions and its strong components.
//!
//! Each mention a term makes is an arc, marked bare, built, or an alternative of a union, which
//! adds no depth, as a bare mention does: from a caller's name to the callee entry name a call
//! maps, and from a question's read to the answered name an arm's content or a term on a site
//! types. A result taken through a call draws a **summary arc** to the taking name from each
//! caller name that reaches the taken name inside the callee ([`summaries`]). No arc runs from a
//! callee's name back to a caller's, since a result returns only to the call that asked for it.
//!
//! A closed walk composes to a name built over itself exactly where a built arc lies inside a
//! strongly connected component: the component's arcs give a walk from the arc's head back to its
//! tail, and the built arc closes it one level deeper. Checking simple cycles alone would miss a
//! swap followed by a wrap, where each call alone repeats. A name a callee inherits unchanged from
//! its lexical chain draws no mapping arc: it joins no two names and builds nothing.
//!
//! The arcs come from every arm the text lays, so the refusal is decided before anything is bound
//! ([the crate's README](../../README.md#refusing-a-recursion-that-grows)). Components are found
//! by Kosaraju's algorithm over explicit stacks, never recursion, since a program may hold
//! thousands of names.

use std::collections::BTreeMap;

use crate::domain::Nesting;
use crate::graph::Edge;
use crate::id::{AnswersId, BodyId, NameId};

/// One mention: `edge`'s term gives `to` a content mentioning `from`.
#[derive(Copy, Clone, Debug)]
pub(super) struct Arc {
    pub(super) from: NameId,
    pub(super) to: NameId,
    pub(super) nesting: Nesting,
    pub(super) edge: Edge,
}

/// One `AnswersName` edge through a call: `name` takes the callee's `takes`.
pub(super) struct Through {
    pub(super) edge: AnswersId,
    pub(super) name: NameId,
    pub(super) takes: NameId,
    pub(super) callee: BodyId,
    /// Each callee entry name the call maps, with the caller names its term mentions.
    pub(super) mapped: Vec<(NameId, Vec<(NameId, Nesting)>)>,
}

/// The summary arcs of every edge through a call, per edge in `throughs` order.
///
/// A summary arc runs to the taking name from each caller name `m` such that a path over
/// `answer_arcs` and the summaries themselves runs from a callee name `s` to the taken name: `s`
/// is an entry name the call maps from a term mentioning `m`, or `m` itself where
/// `outside(through, m)` says the callee does not bind it. It is built where the mapping builds
/// `m` or a step on the path builds. Summaries over recursive bodies are a fixpoint, which stops
/// once no arc is added or upgraded from bare to built.
pub(super) fn summaries(
    names: usize,
    answer_arcs: &[Arc],
    throughs: &[Through],
    outside: impl Fn(&Through, NameId) -> bool,
) -> Vec<Vec<Arc>> {
    let mut found: Vec<BTreeMap<NameId, Nesting>> = vec![BTreeMap::new(); throughs.len()];
    loop {
        // Arcs into each name: the answers' own, then the summaries so far.
        let mut into: Vec<Vec<(NameId, Nesting)>> = vec![Vec::new(); names];
        for arc in answer_arcs {
            into[arc.to.index()].push((arc.from, arc.nesting));
        }
        for (through, arcs) in throughs.iter().zip(&found) {
            for (&from, &nesting) in arcs {
                into[through.name.index()].push((from, nesting));
            }
        }

        // One search per distinct taken name a pass: calls taking the same result share it.
        let mut searched: BTreeMap<NameId, Vec<Option<Nesting>>> = BTreeMap::new();
        let mut changed = false;
        for (through, arcs) in throughs.iter().zip(found.iter_mut()) {
            let reach = searched
                .entry(through.takes)
                .or_insert_with(|| reaching(&into, through.takes));
            let mut add = |from: NameId, nesting: Nesting| match arcs.get_mut(&from) {
                None => {
                    arcs.insert(from, nesting);
                    changed = true;
                }
                Some(held) if *held == Nesting::Bare && nesting == Nesting::Built => {
                    *held = Nesting::Built;
                    changed = true;
                }
                Some(_) => {}
            };
            for (mapped, mentions) in &through.mapped {
                let Some(path) = reach[mapped.index()] else {
                    continue;
                };
                for &(from, nesting) in mentions {
                    add(from, deeper(path, nesting));
                }
            }
            for (index, path) in reach.iter().enumerate() {
                let name = NameId::new(index);
                if let Some(path) = *path
                    && outside(through, name)
                {
                    add(name, path);
                }
            }
        }
        if !changed {
            break;
        }
    }
    throughs
        .iter()
        .zip(found)
        .map(|(through, arcs)| {
            let arcs = arcs.into_iter().map(|(from, nesting)| Arc {
                from,
                to: through.name,
                nesting,
                edge: Edge::Answers(through.edge),
            });
            arcs.collect()
        })
        .collect()
}

/// Built where either is: an alternative of a union, like a bare mention, adds no depth.
fn deeper(a: Nesting, b: Nesting) -> Nesting {
    if a == Nesting::Built || b == Nesting::Built {
        Nesting::Built
    } else {
        Nesting::Bare
    }
}

/// Each name with a path to `target` over `into`'s arcs, built where some path has a built step.
fn reaching(into: &[Vec<(NameId, Nesting)>], target: NameId) -> Vec<Option<Nesting>> {
    let mut reach = vec![None; into.len()];
    reach[target.index()] = Some(Nesting::Bare);
    let mut pending = vec![target];
    while let Some(at) = pending.pop() {
        let path = reach[at.index()].expect("reached");
        for &(from, nesting) in &into[at.index()] {
            let through = deeper(path, nesting);
            let held = &mut reach[from.index()];
            if held.is_none() || (*held == Some(Nesting::Bare) && through == Nesting::Built) {
                *held = Some(through);
                pending.push(from);
            }
        }
    }
    reach
}

/// The first built arc, in the order given, whose ends share a strongly connected component.
pub(super) fn first_growth(names: usize, arcs: &[Arc]) -> Option<Arc> {
    let mut forward = vec![Vec::new(); names];
    for arc in arcs {
        forward[arc.from.index()].push(arc.to.index());
    }
    let component = components(&forward);
    arcs.iter()
        .find(|arc| {
            arc.nesting == Nesting::Built
                && component[arc.from.index()] == component[arc.to.index()]
        })
        .copied()
}

/// Each vertex's strongly connected component, by Kosaraju's algorithm over `forward`'s arcs.
pub(super) fn components(forward: &[Vec<usize>]) -> Vec<usize> {
    let vertices = forward.len();
    let mut backward = vec![Vec::new(); vertices];
    for (from, arcs) in forward.iter().enumerate() {
        for &to in arcs {
            backward[to].push(from);
        }
    }

    // Vertices in the order their depth-first visits finish.
    let mut finished = Vec::with_capacity(vertices);
    let mut visited = vec![false; vertices];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for start in 0..vertices {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        stack.push((start, 0));
        while let Some((vertex, next)) = stack.last_mut() {
            if let Some(&to) = forward[*vertex].get(*next) {
                *next += 1;
                if !visited[to] {
                    visited[to] = true;
                    stack.push((to, 0));
                }
            } else {
                finished.push(*vertex);
                stack.pop();
            }
        }
    }

    // Over the reversed arcs, latest finish first: each search stays inside one component.
    let mut component = vec![usize::MAX; vertices];
    let mut count = 0;
    let mut pending = Vec::new();
    for &start in finished.iter().rev() {
        if component[start] != usize::MAX {
            continue;
        }
        component[start] = count;
        pending.push(start);
        while let Some(vertex) = pending.pop() {
            for &from in &backward[vertex] {
                if component[from] == usize::MAX {
                    component[from] = count;
                    pending.push(from);
                }
            }
        }
        count += 1;
    }
    component
}
