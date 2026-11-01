## Workflow
- Prefer minimal, focused edits. Do not start bulk or exploratory edits without explicit confirmation; if a change looks overly complex, say so and suggest an alternative before continuing.
- Match the process to the size of the change: for a substantial or multi-step change, propose a plan and wait for approval before editing; for a small, well-understood edit, just make it.
- After smoke testing features or bug fixes, try to create a verifiable unit test based on the smoke test.
- Top-level structs and free functions come with comments explaining their purpose. The README contains an overview of the architecture. When modifying code, make sure these docs stay up-to-date and brief.
- When using subagents:
  - prefer Sonnet for purely mechanical tasks and searches
  - prefer Opus if there may be ambiguity in the task or search
  - launch every independent job in a single message so they run concurrently.
  - split a wide refactor: one foundation agent changes the hub files and stops, leaving callers broken, and reports the final signatures and the per-file `cargo check` errors; then parallel agents fix disjoint sets of caller files.
  - tell implementation agents never to `git checkout` or `git restore` tracked files; a stray revert destroys an earlier phase's uncommitted work.
  - re-derive an agent's rationale and its before/after numbers (against the merge base) before relaying them. In a wide sweep, report a compact per-file summary, not each agent's report.
- Never discard uncommitted tracked work (`git restore`, `git checkout -- <path>`, `git stash`) without asking, including a subagent's output or work a ruling superseded. Each `git commit` needs its own approval: "commit X" authorizes X only.
- Meet every acceptance criterion fully. Only the user may relax or split one.
- In a phased plan, front-load the shared plumbing a later phase plugs into: no phase should rewrite what an earlier one wrote.
- When the user overrules a decision mid-implementation, fold the ruling and the verified findings into the plan, delete the branch and restart; don't unwind the work in place. When the user discards an attempt, don't read or reuse its commits, results or leftovers.
- For a change spanning code and docs, verify the code path before writing the docs.
- One run of an expensive verification (`tools/verify.sh`, the Miri slate) decides pass/fail. Land every code and test edit, new slate tests included, before it. Don't re-run a clean result, re-run to reshape or truncate output, or revert a fix to prove a regression test bites.

## Project Context
- Koan is a pre-release language with NO users; do not invent backward-compatibility concerns or migration paths in design proposals.
- Write documentation (tutorial/, README.md) from the user's perspective, not the implementer's.

## Language Rules
- Keywords (`LET`, `PRINT`, `->`, `OF`) are fixed syntax, meaningful only in combination with the rest of an expression shape. Never call a keyword a function or imply it is callable or looked up; frame a builtin as an expression shape of keywords and typed slots.
- A single uppercase letter (`T`, `K`) or capitals with digits (`K9`) is an ambiguous token and a parse error. In snippets and metavariables use a name with a lowercase letter: `Elem`, `Key`, `Mod`.
- `PRINT` returns the string it printed.
- A dispatch placeholder or park keys on the full bucket key (every keyword), never the lead keyword alone, and is uniform across binders that produce functions.
- No global or run-rooted mutable state, such as a run-wide registry: it hurts understandability and precludes concurrency. Home shared lookup data on the value or type it describes.
- A record that reaches another scope stores the *name* and resolves it at the use site (through `USING`), never a resolved reference. A new lifetime on a stored type is the tell.

# Rust Conventions
- When refactoring types/lifetimes, verify with `cargo build` after each step rather than batching multiple type changes.
- Prefer the simplest design; avoid OnceLock or complex synchronization unless explicitly needed.
- Find joy in deleting unused or unnecessary code.
- When a refactor orphans a helper, delete it and its tests and take the purer idiom; never keep code alive to satisfy a dead-code check.
- `src/` production code carries no `unsafe`; test-only `unsafe` lives in `src/tests.rs`. Removing an `unsafe` is always worth it: "audited", "encapsulated" and "irreducible" are not resting states. Prefer an abstraction that turns the bug into a compile error over one that only relabels the `unsafe`. When every safe design has a real cost, offer a soundly encapsulated `unsafe` (one private writer, a soundness argument, a Miri test) and let the user rule.
- A runtime check standing in for a compile-time guarantee is a stopgap: accept one only with a paired roadmap item, and never describe it as enforcement.
- Region bookkeeping is `Copy` and `Drop`-free, laid in the region's bump, so a frame's death is O(1). Cycles inside one arena are fine.
- `OnceCell::get_or_init`'s returned `&T` descends from a unique borrow; a later `get()` plus an interior write through it is UB under tree borrows. Initialize such cells eagerly.
- To carry new data through delicate core (scheduler, dispatch, build), generalize an existing carrier (retype an `Option` into an enum) rather than threading a parallel field, and present the mechanism options first.
- When a shared door is too costly at one call site, put a fact on the data that lets every caller stop early; never special-case one caller.
- `cargo check` is the ground truth for compile errors, not the LSP. Finish editing a file before querying the LSP about it, and take line/character positions from a fresh read.

## Design Discussions
- When the user asks a conceptual or 'should we?' question, answer it first — do NOT immediately start implementing.
- For pattern-dispatch / signature work, confirm the user's syntax intent before proposing new KType variants.
- Syntax and semantics forks are the user's to decide: enumerate the options with their consequences and ask, even mid-implementation.
- Before surfacing recorded forks, test each premise against the end-state docs, downstream acceptance criteria and the code; only genuine rulings go to the user. Before deferring a direction to a later item, write the smallest program the current item allows that hits it.
- Record a decision at the scope the user stated it; surface its consequences as questions.
- When the user asserts a language invariant, tests that contradict it are bugs to fix.
- An old tutorial's output is not a spec: list each rule a snippet implies as a question, and reason from the language's invariants, not its representation.
- Push back when a request's justification no longer holds under a revised plan.
- Every feature the user floats must land in the doc tree before the session ends.
- For work or design questions touching types — `lattice/src/types` (koan's `type_lattice`), the elaborator's type channel, dispatch's static types or selection, a value's carried type — invoke the **type-system skill** first. It holds the lattice's invariants and the checklist a change must answer.

## Documentation
- Keep documentation updated and as concise as possible. Do not sacrifice grammar for brevity.
- Say "expression shape", never "form"; "operator run", never a bare "run"; and qualify "node" (type node, arena node, composition, settlement).
- Design docs (`design/`, module READMEs) pin the intended end state. Rewrite them at plan time, with the user's permission, never during implementation; if implementation shows one is wrong, stop and ask. The shepherd may edit them at ship.
- For work touching `README.md`, `tutorial/`, a module's `README.md` design doc, `design/*.md`, or `roadmap/*.md` (including the `roadmap/README.md` index), invoke the **documentation skill** — it owns the doc-tree partition rules, deletion-vs-edit semantics for shipped roadmap items, and the `doclinks check` gating workflow.
- Source-file comments stay here because they're maintained during implementation, not at the doc-update phase:
  - Top-of-file comments: explain the code in the file, assumptions it makes, and how it's related to the code in other files. Link to design docs where needed. Update these as you go.
  - Inline comments: keep these to 3-4 lines. Extra content should go in the top-of-file comments or in design docs; link when needed. Update these as you go.

## Repo Conventions
- Tests live as `mod tests;` inside the source module's own subdirectory — `foo/tests.rs`, split to `foo/tests/<topic>.rs` only once the single file is genuinely hard to navigate (~500+ lines). Never a sibling `foo_tests.rs`.
- Scratch notes, drafts and plan files belong in `scratch/` at the repo root (gitignored). Plan mode writes to `.claude/plans/`, which is tracked — keep the canonical plan in `scratch/` and delete the `.claude/plans/` copy when planning ends.
- Spell words out in koan identifiers, variants and tags: `error`, not `err`.
- Local state files that record HEAD's SHA on each run (trend logs, complexity baselines) are gitignored, not tracked — tracking one doubles every commit.
- Run `cargo fmt --all` before wrapping up and keep the whole-tree result; do not revert fmt-only hunks in unrelated files to keep a diff scoped. `cargo fmt` is not part of the verify slate.
