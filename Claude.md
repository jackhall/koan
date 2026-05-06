## Always
Be concise. Prefer minimal, focused edits. Do not start bulk or exploratory edits without explicit confirmation. 

## Workflow
- If you think that given change may be overly complex, tell me so and explain why, then ask me whether to continue anyway. Suggest an alternative if one comes to mind. Do not start bulk or exploratory edits without explicit confirmation.
- After smoke testing features or bug fixes, try to create a verifiable unit test based on the smoke test.
- Always propose a plan before making changes; wait for approval before editing. 
Top-level structs and free functions come with comments explaining their purpose. The README contains an overview of the architecture. When modifying code, make sure these docs stay up-to-date and brief. Do not sacrifice grammar for brevity. 

## Project Context
- Koan is a pre-release language with NO users; do not invent backward-compatibility concerns or migration paths in design proposals.
- Write documentation (TUTORIAL.md, README.md) from the user's perspective, not the implementer's.

# Rust Conventions
- Use `vec![...]` (with `!`) for vec literals — common typo to watch for.
- When refactoring types/lifetimes, verify with `cargo build` after each step rather than batching multiple type changes.
- Prefer the simplest design; avoid OnceLock or complex synchronization unless explicitly needed.

## Design Discussions
- When the user asks a conceptual or 'should we?' question, answer it first — do NOT immediately start implementing.
- For pattern-dispatch / signature work, confirm the user's syntax intent before proposing new KType variants.

## Documentation
- Keep documentation updated and as concise as possible.
- Do not sacrifice grammar for brevity.
- For work touching `README.md`, `TUTORIAL.md`, `ROADMAP.md`, `design/*.md`, or `roadmap/*.md`, invoke the **documentation skill** — it owns the doc-tree partition rules, deletion-vs-edit semantics for shipped roadmap items, and the `doclinks check` gating workflow.
- Source-file comments stay here because they're maintained during implementation, not at the doc-update phase:
  - Top-of-file comments: explain the code in the file, assumptions it makes, and how it's related to the code in other files. Link to design docs where needed. Update these as you go. After each PR is code-complete and tested, check to see if any of this info belongs in design docs or the roadmap instead — if it does, the documentation skill handles the move.
  - Inline comments: keep these to 3-4 lines. Extra content should go in the top-of-file comments or in design docs; link when needed. Update these as you go.
