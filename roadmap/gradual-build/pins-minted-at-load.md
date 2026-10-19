# Every pinned path minted at the load

**Problem.** A member read through a name chain pins each head parameter its
operand's static upper end leaves unpinned to the path through it
([`pinned`](../../src/dispatch/statics/paths.rs)), and the static pass mints
that path's entry keyed on the application it reads through. A resumption that
binds a name the application reads computes another application, misses the
key, and mints. Over `SIG Two FOR ALL #[Left Right] = #[(VAL left :Left)
(VAL right :Right)]`, the body of `EXPR FOR ALL #[Elt] #(WRAP x :Elt AND
m :(Two WITH {Left = Elt}))` reads `m.right`, which pins `Right` through
`Two WITH {Left = Elt}`. A resumption that binds `Elt` reads through
`Two WITH {Left = Number}` and panics (`a resumption mints no path entry`). A
specialization's step would panic the drain the same way.

**Acceptance criteria.**

- A resumption mints no path entry for a member read's pin, and binding a name
  an operand's declared application reads resumes without refusing: the case
  above is a test.
- The resumption laws' generator reads a member through a module parameter
  whose declared application pins one head parameter at a run-bound name and
  leaves another unpinned.

**Directions.**

- *How a resumption finds a pin's entry* — open. The load may record, per
  member read, the entry its pins read, so a resumption reads that entry by
  site. Its `through` is then the load's, over lexical variables, which each
  reading shape locates as it does for a spelled path. Alternatively, a
  resumption finds the load's entry whose `through`, with the binding
  substituted, is its own upper end.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Activation typing](activation-typing.md) — a specialization's resumption reads every pin its binding wakes.
