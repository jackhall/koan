# The graph written out and read back

**Problem.** A `Graph` is made only by `Text::lay`
([graph.rs](../src/graph.rs)), and its fields are private. Nothing writes a
laid graph out or rebuilds one without a text to lay, so a run that resumes
from a build has to describe and lay its text again.

**Acceptance criteria.**

- A laid `Graph` exports to plain data: its ids, sites, owners, nodes, edges,
  site order, [text components](../README.md#instances) and each
  `BindingNode`'s handle fields as integers, and each mapping's, arm's and
  site edge's term and each declared ask's `NameQuestion` as the facts' own
  values.
- Importing that data rebuilds a `Graph` without a `Text` and without
  running `lay`.
- A property test round-trips every generated text that lays. The imported
  graph has the same owners, nodes, `BindsAfter` edges, branches, declared
  asks, handle widths, site order and text components as the laid one, and a
  `Bound` over it gives the same handles and answers.
- Instances are not part of the export: it reads a `Graph` alone, and no
  `Bound`.

**Directions.**

- *Who writes a `Term` and a `NameQuestion` — open.* The crate takes a writer
  and a reader for them from the facts, or the export hands back the values
  for the embedder to write. Recommended: the export hands them back, which keeps
  the crate free of a format.
- *Whether import re-checks — open.* Import can re-run the read, edge and
  growth checks over the imported data, or trust data the build wrote.
  Recommended: trust it, since the build image already refuses an image whose
  source digest is not the program's.

## Dependencies

**Requires:** none.

**Unblocks:**

- [The build image](../../roadmap/one-build/build-image.md) — the image writes
  the graph and resumes from it without laying.
