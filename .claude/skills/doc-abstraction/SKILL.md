---
name: doc-abstraction
description: Use when extracting refactor candidates by reading design docs and source comments — finding concepts that span 3+ docs or source files without a single owner. Cross-doc companion to `rust-abstraction` (file-level) and `modgraph` (graph scoring); candidates surfaced here typically feed into one of those for scoring.
---

# doc-abstraction

Find refactor candidates by reading prose, not source structure. A concept described across many docs or restated in many comment blocks usually means the underlying code lacks a home.

## Signals

- **Concept restated in 3+ design docs** without one canonical "owner" doc.
- **Same invariant reasserted across N source files** as docstrings/comments — load-bearing rule lives only in prose.
- **A protocol with named participants** (e.g. "every X must Y") but no trait, type, or module that enumerates them.
- **Cross-doc reference chains** where doc A links to doc B for a concept B says is "primarily described in" doc C.
- **Co-cited module cluster:** the same set of source modules appears together in multiple docs or source-file headers. Recurring co-citation = an unnamed protocol binding them.

## Not signals

- A concept described once thoroughly in one doc — that's good docs, not coupling.
- Repetition of *terms* (e.g. "scope", "arena") without repetition of *rules*.

## Workflow

1. Run `python3 tools/doclinks.py signals` for the mechanical pass — co-cited src triples, backref density, comment-density spikes, recurring n-gram phrases, reference chains, unowned concepts, doc hubs. JSON to stdout.
   - Pair with `python3 tools/doclinks.py gap` to rank src-file *pairs* by `(docs_co_citing + shared_phrases) / (1 + structural_edges)` — concepts the docs see that the cargo-modules graph doesn't. High gap = candidate seam (hunt only); confirm with `modgraph_rewrite.py item` before refactoring, since a high-gap pair can still be net-negative under structural scoring.
2. Filter the digest for semantic relevance (the tool can't): does each signal name a real concept, or is it incidental co-citation?
3. Scan the full doc tree (not just doclinks's hits) for what the tool can't see: prescriptive "every X must Y" rules with participants, implicit references ("the dispatch model" with no link), owner-doc claims in intros. Skip any signal that doclinks already covers.
4. Cross the two: do step 3's findings name the same concepts doclinks flagged, or surface new ones? Decide which (if any) doc *owns* each.
5. Propose one of three seams:
   - **Code seam:** a module / trait / type that names participants.
   - **Doc seam:** a single canonical page the others link to.
   - **Pointer seam:** every source file gets a doc link to the contract page.
6. Report each candidate as: concept, docs it spans, source files it spans, proposed seam, risk, payoff.

## Reject

- Empty-wrapper protocol traits (one impl, no shared logic).
- Doc consolidation that just moves words around without naming participants.
