---
name: rust-abstraction
description: Use when deciding whether a long Rust file in the koan repo has an extractable seam — and what *shape* the extraction should take. Judgment-side companion to `rust-refactor` (mechanical moves) and `modgraph` (partition scoring). Reach for it when asked "can this be simplified?", "are there seams here?", "what should come out of this file?".
---

# rust-abstraction

Decide whether a long Rust file has a seam worth extracting, and what shape the extraction should take. "No seam, skip this file" is a valid output — don't manufacture work.

## Check the cheap fixes first

Before looking for abstractions, see if a mechanical fix handles it. If yes, do that and stop.

- **Tests inline?** Measure prod vs. test lines. If tests are >20% of the file and live in `#[cfg(test)] mod ...` blocks, lift them to `foo/tests.rs` per the project convention. That alone often resolves "this file is too long."
- **Hand-written `Clone`/`Debug` that could `derive`?** Replace and stop.
- **Dead code from a half-done refactor?** Delete and stop.

These aren't abstraction work; they're cleanup. The rest of the skill only kicks in if cleanup doesn't get you there.

## Seam smells

Look for these. One strong signal is enough; three weak ones is not.

- **Inline `struct`/`enum` in a function body.** The author already factored the *concept* but didn't lift the *boundary*. Promote it.
- **Repeated loop or match shape with varying bodies.** N near-identical `for/match` walks differing only in body or predicate → consolidate.
- **Read-only narrow access to a wide type.** A method that touches one or two fields of a large struct and never the rest. That slice *is* the boundary.
- **Load-bearing invariant living as a docstring.** A rule that must be preserved (cache-safety, ordering, "don't recurse here") is one rename away from being lost — promote it to a type name.

If none of these fire after a real read of the file, the honest answer is **"no seam — skip."** Say so out loud.

## When picking a shape

- **No empty wrappers.** If `Foo::new(thing).do_x()` just renames `Thing::do_x(thing)`, reject it. The test: *can you state in one sentence what the new type guarantees?* If not, it's a wrapper.
- **Multi-file `impl` blocks are fine.** A method that belongs in a sibling file by *concern* but reads better as `f.method()` than `pick::method(&f, ...)` — keep it as a method, lift it via `impl<'a> Foo<'a>` in the new file.

## Out of scope

- **Rearranging modules** - that's `modgraph`. 
- **The mechanical move itself** — that's `rust-refactor`. This skill is judgment; that one is execution.

## Workflow

1. Read the file end to end (not skim).
2. Run the **cheap-fixes** list. If one applies, do it and stop.
3. Look for **seam smells**. Score honestly — weak signals are not seams.
4. If you find a seam: name the abstraction, state its one-sentence guarantee, propose it to the user. If you don't: report "no seam, skip" and move on.
5. Hand approved seams to `rust-refactor`.
