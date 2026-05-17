---
name: rust-abstraction
description: Use when deciding *where* to draw a boundary in a long Rust file or function — finding seams to extract, choosing the right shape (struct, enum, iterator, classifier), and avoiding empty wrappers. Judgment-side companion to `rust-refactor` (which covers the mechanical execution of moves and rewrites). Reach for this before `rust-refactor` whenever the user asks "can this be simplified?", "are there seams here?", or "what abstraction belongs here?".
---

# rust-abstraction

Companion to `rust-refactor`. That skill is about *how* to perform moves, renames, and rewrites mechanically. This one is about *what* to lift out and *how to shape* it once you've decided the file is too long or too tangled.

Use the signals below to find candidate seams, the rules to choose the abstraction's shape, and the "what not to extract" list to resist over-engineering. When you've decided, hand off to `rust-refactor` for the actual move.

## Signals that mark a seam

1. **Inline types inside a long function.** An `enum` or `struct` defined inside a function body is a tell: the author already factored out the *concept* but couldn't lift the *boundary*. Promote the type to module scope first; the function shrinks naturally.

2. **"Step N" phase comments.** Numbered phases inside one function are unextracted function names the author already wrote. Each phase comment is a candidate function name.

3. **Repeated loop or match shape with varying bodies.** N copies of the same `while let` / `for` / filter pipeline that differ only in the body or predicate → iterator + closure. N match arms that differ only in how they assemble a final value → classifier function returning a sum type.

4. **Free functions that don't touch the host type.** Pure structural walkers (`fn walk(&Foo) -> ...`) misfiled in a high-coupling module. Often the cheapest lift in the file because they have no scope/handle/state dependency to thread through.

5. **Read-only narrow access to a wide type.** A function uses one or two fields of a large struct and never the rest. That narrow slice *is* the API boundary, and justifies a separate module that consumes only what it needs.

6. **Load-bearing invariants living as comments.** A rule that must be preserved (cache-safety, ordering, atomicity, "don't recurse here") sitting in a docstring is one rename away from being lost. Promote it to a type name.

## Rules for choosing abstractions

1. **Data + behavior, not wrappers.** A `Foo::new(thing).do_x()` that just renames `Thing::do_x(thing)` adds zero semantic content — reject it. Extract only when the new type encapsulates an algorithm, an invariant, or a non-obvious decision. The test: can you state what the new type *guarantees* in one sentence? If not, it's a wrapper.

2. **Name load-bearing invariants as types.** A type name is harder to ignore than a comment. Prefer a one-method struct with a meaningful name over a free fn carrying the rule in its docstring — future changes have to grapple with the name, not just notice a remark.

3. **Policy-free shape types.** When several callers diverge on what the same outcome means, the outcome enum should describe *what happened*, not *what to do*. (e.g. `{ Picked | Tie | Empty }` is shape; "tie means ambiguous here, fall through there" is policy.) Policy stays at call sites; shape stays stable across them.

4. **Iterators over collectors when iteration is enough.** Prefer yielding borrowed views to allocating + cloning into a `Vec`, especially when consumers have early-exit potential.

5. **Bundle co-varying positional args into a struct.** If three or more parameters are always built together at every call site, they're already a value — give it a name. Functions taking 5+ positional args are often hiding a struct.

## What not to extract

- Patterns where each instance closes over a different signature or type. A macro or generic helper would obscure legible symmetry. Three similar lines beats a premature abstraction.
- Thin shims (3–5 lines) that exist to give a type a clean read surface. Pushing callers through them leaks the underlying façade.
- Speculative abstractions for hypothetical second callers. Wait for the third instance — the first two might be coincidence; the third is a pattern.

## Workflow

1. Read the long file end to end. Don't skim — the signals above are easy to miss in a partial read.
2. List candidate seams with one-line justifications grounded in the signals.
3. For each seam, draft the *abstraction* (type name, method names, one-sentence invariant). Apply the four rules. Discard any that fail the "what does this guarantee?" test.
4. Present the seams to the user ranked by leverage (largest single concern with the cleanest boundary first). Get agreement before moving.
5. Hand off to `rust-refactor` for the mechanical work: create the new file, move items, mark `pub` what crosses boundaries, `cargo build`, `cargo clippy`, `verify`.
6. After each extraction, re-read the host file. Earlier seams often become clearer once the first one is out.

## When this skill does *not* apply

- The file is long but homogeneous (e.g. a large match statement over an enum). Length alone isn't a seam — look for the signals above.
- The user already knows what to extract and just needs the move performed. Go straight to `rust-refactor`.
- The boundary question is about module dependency structure rather than content. Use the `modgraph` skill to score the partition.

## See also

- `rust-refactor` — mechanical execution of the move once a seam is chosen.
- `modgraph` — scoring proposed module reshuffles against the live dependency graph.
- `verify` — confirms the refactor didn't regress tests, clippy, or coverage.
