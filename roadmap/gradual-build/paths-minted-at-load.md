# Every spelled path minted at the load

**Problem.** The type channel mints a path's entry where the elaborator reaches its spelling
([`Pass::path`](../../src/elaborate/channel.rs)), and the elaborator stops at the first name it
reads as unknown. So the load mints no entry for a path a type binder spells after a run-bound
name: in `EXPR FOR ALL #[Elt] #(WRAP x :Elt m :Counter)`, the body's
`LET Both = :{a :Elt b :(m.Carrier)}` declares unknown at `Elt` and never reaches `m.Carrier`.
A resumption that binds `Elt` reaches the spelling, finds no entry the load minted, and panics
(`a resumption mints no path entry`). A specialization's step would panic the drain the same way.

**Acceptance criteria.**

- The load mints an entry for every path a type expression spells, wherever the elaborator stops.
- A resumption mints no path entry, and binding a name a type binder reads before a path resumes
  without refusing: the case above is a test.
- The resumption laws' generator spells a path after a run-bound name in a type binder.

**Directions.**

- *How the load finds every spelling* — open. Recommended: the shape builder records each
  spelling whose root is a value token and whose label is a type token — a path by its tokens —
  and the type channel mints each in program order as it opens the shape, before it types
  anything there. The alternative lets the elaborator read past an unknown name, which risks a
  refusal the load does not make today.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Activation typing](activation-typing.md) — a specialization's resumption reaches every spelling its binding wakes.
