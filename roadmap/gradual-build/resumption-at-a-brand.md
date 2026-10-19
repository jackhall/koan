# A resumption at any brand

The load's two passes, resumable under a binding wherever a step runs them.

**Problem.** A resumption — the type channel's walk
([`resume_channel`](../../src/elaborate/channel.rs)) then the static pass's
([`resume_statics`](../../src/dispatch/statics/resume.rs)) under a binding — runs only inside the
load, at the program's own lifetime: it writes through program storage's writer, interns through the
load's chain, and lays a `Settled` there ([`Loaded::resume`](../../src/program/substrate.rs), which
only tests call). It cannot run in a specialization's step, whose writer and chain are the
specialization's own. The facts vocabulary is why: a content tree, a knot node and a binder hold a
reference to their body's shape ([`tree.rs`](../../src/scope/tree.rs)), and a path variable holds a
reference to its path entry, which holds a write-once cell for its resolution
([`typed.rs`](../../src/scope/typed.rs)). So every static type, use record, `Statics` and `Settled`
is invariant in program storage's lifetime, and one laid in a shorter-lived region does not
typecheck. The passes' rules also write the shape's cells directly through the fact stores, in the
same code a resumption runs.

**Acceptance criteria.**

- The facts vocabulary names a body by its segment and a path variable by its chain root's segment
  and its level, and holds no reference into program storage. Every vocabulary type is covariant
  in its one lifetime, which compile-time probes assert for `StaticType`, `Statics`, `ShapeFacts`,
  `Resolution` and `Settled`.
- Program storage finds a body's shape by its segment.
- A path's resolution rests in a write-once table on its chain root's shape, keyed by level, and a
  path entry holds no cell.
- The load's passes and a resumption run one set of rules; they differ only in the fact stores'
  write half, which writes the shape's cells in the load and the resumption's own tables in a
  resumption.
- The passes' rules hold no writer: they lay into the walk's scratch, and the write half copies
  each fact it keeps into its output — the shape's cells at the load, the `Settled` in a
  resumption. A resumption lays only what its `Settled` keeps.
- A resumption's rules intern through the scratch chain they are handed, and its write half adopts
  into the output chain each type a kept fact names, so the branch it writes gains only the types
  its `Settled` names. Every handle a `Settled` names resolves through the output chain.
- `resume_channel` and `resume_statics` run under any step's brand, laying their `Settled` through
  the writer they are handed. A test step resumes a binding naming a type the root lacks, at its
  own brand, and the root is unchanged.
- The resumption laws in the dispatch suite hold.

**Directions.**

- *The vocabulary* — decided. By name: a body by its segment, a path variable by its chain root
  and level, never a stored reference, rather than a second lifetime on every vocabulary type.
- *One rule set* — decided. The fact stores split into a read half and a write half, a trait the
  load and a resumption each implement, so the rules are written once.
- *Where the rules lay* — decided. In the walk's scratch, with a resumption's types in the step's
  scratch branch; the write half copies what it keeps, as a caching body's exit adopts what leaves
  it. The copy is checked by lifetimes; a missed adopt is not, so a debug check and a test hold it.
- *Where a shape finds another by segment* — decided. Every shape reaches the load's one table,
  since the run's path readers hold an activation alone.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Activation typing](activation-typing.md) — a specialization's step runs the resumption at its own brand.
