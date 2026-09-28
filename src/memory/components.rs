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
    fn filled<'a, T: Clone>(scratch: BumpAllocator<'a>, count: usize, value: T) -> BumpVec<'a, T> {
        let mut cells = BumpVec::with_capacity_in(count, scratch);
        cells.resize(count, value);
        cells
    }

    let count = edges.len();
    let mut next_index = 0;
    let mut indices: BumpVec<'a, Option<usize>> = filled(scratch, count, None);
    let mut lowlink = filled(scratch, count, 0);
    let mut on_stack = filled(scratch, count, false);
    let mut stack = BumpVec::with_capacity_in(count, scratch);
    let mut components = BumpVec::with_capacity_in(count, scratch);
    // The walk's own call stack, held here rather than on the thread's: each entry is a node being
    // visited and the next of its edges to follow, so a chain as long as the graph costs no depth.
    let mut visiting: BumpVec<'a, (usize, usize)> = BumpVec::with_capacity_in(count, scratch);
    for root in 0..count {
        if indices[root].is_some() {
            continue;
        }
        visiting.push((root, 0));
        while let Some(&mut (v, ref mut edge)) = visiting.last_mut() {
            if *edge == 0 && indices[v].is_none() {
                indices[v] = Some(next_index);
                lowlink[v] = next_index;
                next_index += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            if let Some(&w) = edges[v].get(*edge) {
                *edge += 1;
                match indices[w] {
                    None => visiting.push((w, 0)),
                    Some(w_index) if on_stack[w] => lowlink[v] = lowlink[v].min(w_index),
                    Some(_) => {}
                }
                continue;
            }
            // Every edge of `v` is followed: close it, and fold its lowlink into its caller's.
            visiting.pop();
            if lowlink[v] == indices[v].expect("v was indexed on entry") {
                let mut component = BumpVec::new_in(scratch);
                loop {
                    let w = stack.pop().expect("the stack holds v");
                    on_stack[w] = false;
                    component.push(w);
                    if w == v {
                        break;
                    }
                }
                components.push(component);
            }
            if let Some(&(caller, _)) = visiting.last() {
                lowlink[caller] = lowlink[caller].min(lowlink[v]);
            }
        }
    }
    components
}

#[cfg(test)]
mod tests;
