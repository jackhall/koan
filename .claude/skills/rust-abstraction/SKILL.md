---
name: rust-abstraction
description: Use when deciding whether a long Rust file in the koan repo has an extractable seam — and what *shape* the extraction should take. File-level companion to `modgraph` (complexity scoring). Reach for it when asked "can this be simplified?", "are there seams here?", "what should come out of this file?".
---

# rust-abstraction

Decide whether a long Rust file has seams worth extracting, and if there are, how to extract each. 

## Check the cheap fixes first

Before looking for abstractions, see if a mechanical fix handles it. 

- **Tests inline?** Measure prod vs. test lines. If tests are >20% of the file and live in `#[cfg(test)] mod ...` blocks, lift them to `foo/tests.rs` per the project convention. That alone often resolves "this file is too long."
- **Hand-written `Clone`/`Debug` that could `derive`?** Replace and stop.
- **Dead code from a half-done refactor?** Delete and stop.


## Seam smells

Look for these if there are no cheap fixes. One strong signal is enough; three weak ones is not.

- **Inline `struct`/`enum` in a function body.** The author already factored the *concept* but didn't lift the *boundary*. Promote it.
- **Repeated loop or match shape with varying bodies.** N near-identical `for/match` walks differing only in body or predicate → consolidate.
- **Read-only narrow access to a wide type.** A method that touches one or two fields of a large struct and never the rest. That slice *is* the boundary.
- **Load-bearing invariant living as a docstring.** A rule that must be preserved (cache-safety, ordering, "don't recurse here") is one rename away from being lost — promote it to a type name.

## When picking a shape

Your goal is to make the code easier to read and safer to edit. Try to encapsulate data and invariants.

- **No empty wrappers.** If `Foo::new(thing).do_x()` just renames `Thing::do_x(thing)`, reject it. The test: *can you state in one sentence what the new type guarantees?* If not, it's a wrapper.

## Workflow

1. Read the file end to end (not skim).
2. Run it by the **cheap-fixes** list, then the **seam smells** list. Give honest judgment - weak signals are not seams.
3. If you find one or more seams, report the name of the proposed abstraction and (briefly) how it improves the code.
4. f you don't find a seam: report "no seams found" and move on.
