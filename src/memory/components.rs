//! **Strongly connected components** over an index graph, staged in a bump.
//!
//! A node is an index, and `edges[i]` lists the nodes `i` references. The graph comes in as borrowed
//! runs and the components come out as runs of node indices, so the walk names nothing of what the
//! nodes stand for: the type lattice runs it over the members of a recursive group, and a scope runs
//! it over a body's bindings.
//!
//! See [README.md § Strongly connected components](README.md#strongly-connected-components).

use super::bump::{BumpAllocator, BumpVec};

/// Tarjan's strongly connected components over `edges` (`edges[i]` = the nodes `i` references).
///
/// Components come back in the algorithm's emission order, which is a reverse topological order of
/// the condensation: a component is emitted only after every component it references. Every buffer
/// stages in `scratch`.
pub fn strongly_connected_components<'a>(
    scratch: BumpAllocator<'a>,
    edges: &[&[usize]],
) -> BumpVec<'a, BumpVec<'a, usize>> {
    struct State<'e, 'a> {
        scratch: BumpAllocator<'a>,
        edges: &'e [&'e [usize]],
        index: usize,
        indices: BumpVec<'a, Option<usize>>,
        lowlink: BumpVec<'a, usize>,
        on_stack: BumpVec<'a, bool>,
        stack: BumpVec<'a, usize>,
        components: BumpVec<'a, BumpVec<'a, usize>>,
    }

    fn strong_connect(state: &mut State<'_, '_>, v: usize) {
        state.indices[v] = Some(state.index);
        state.lowlink[v] = state.index;
        state.index += 1;
        state.stack.push(v);
        state.on_stack[v] = true;
        for edge in 0..state.edges[v].len() {
            let w = state.edges[v][edge];
            match state.indices[w] {
                None => {
                    strong_connect(state, w);
                    state.lowlink[v] = state.lowlink[v].min(state.lowlink[w]);
                }
                Some(w_index) if state.on_stack[w] => {
                    state.lowlink[v] = state.lowlink[v].min(w_index);
                }
                Some(_) => {}
            }
        }
        if state.lowlink[v] == state.indices[v].expect("v was just indexed") {
            let mut component = BumpVec::new_in(state.scratch);
            loop {
                let w = state.stack.pop().expect("the stack holds v");
                state.on_stack[w] = false;
                component.push(w);
                if w == v {
                    break;
                }
            }
            state.components.push(component);
        }
    }

    fn filled<'a, T: Clone>(scratch: BumpAllocator<'a>, count: usize, value: T) -> BumpVec<'a, T> {
        let mut cells = BumpVec::with_capacity_in(count, scratch);
        cells.resize(count, value);
        cells
    }

    let count = edges.len();
    let mut state = State {
        scratch,
        edges,
        index: 0,
        indices: filled(scratch, count, None),
        lowlink: filled(scratch, count, 0),
        on_stack: filled(scratch, count, false),
        stack: BumpVec::with_capacity_in(count, scratch),
        components: BumpVec::with_capacity_in(count, scratch),
    };
    for v in 0..count {
        if state.indices[v].is_none() {
            strong_connect(&mut state, v);
        }
    }
    state.components
}

#[cfg(test)]
mod tests;
