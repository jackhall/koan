# Spliced arguments render as type digests

A dispatch diagnostic naming an already-evaluated argument prints a 32-hex-digit
type digest where the reader expects a value.

**Problem.** `WorkingPart::Spliced` is the slot shape an argument takes once it
has evaluated, and the expression-summary path renders one through
`write_spliced_summary` in
[working.rs](../../src/machine/model/ast/working.rs), whose `Type` and `Object`
arms both write `0x{:032x}` off the carried type's digest. So a failed dispatch
over an evaluated argument reads:

```
error: dispatch failed for TWICE 0xda8a6addc7627c0fae4be842dfbe13ab: no matching
function: an argument evaluated before dispatch; write #(…) to pass the code itself
```

The digest names neither the argument's spelling nor its value, and it is the
argument's *type* digest, so two distinct values of one type render
identically. The renderer that would do this correctly already exists —
`Carried::write_summary` in
[carried.rs](../../src/machine/model/values/carried.rs) writes an object's
summary and a type's name — but `write_spliced_summary` cannot reach it: the
summary path is threaded with a `&LabelInterner`, while `Carried::write_summary`
takes the whole `&RunRegistries`. The digest is the fallback that plumbing
forces.

Every caller of `WorkingExpression::summarize` already holds the registries
(`expr.summarize(&ctx.registries().labels)` is the shape at each dispatch-failure
site), so the narrower parameter buys nothing. Bare `(…)` arguments now evaluate
before their parent dispatches, so `Spliced` parts reach failed dispatches far
more often than they did — the wart sits directly beside the forgotten-quote hint
that names an evaluated argument as the cause.

**Acceptance criteria.**

- A dispatch diagnostic naming an evaluated argument renders that argument's
  value surface — the same text `PRINT` writes for it — not a digest.
- Two distinct values of one type render distinguishably in a dispatch
  diagnostic.
- `write_spliced_summary` has no digest fallback arm: every `Carried` arm routes
  through `Carried::write_summary`.
- A test pins the rendered text of a dispatch failure whose argument is an
  evaluated group, against the argument's own printed form.

**Directions.**

- *Renderer — decided.* Route the `Spliced` arm through
  `Carried::write_summary`, the existing view. No new rendering surface; the
  arms it covers are exactly the arms `write_spliced_summary` matches on.
- *Plumbing — decided.* Widen the expression-summary path from `&LabelInterner`
  to `&RunRegistries`. Every production caller already has the bundle in hand,
  so the widening is mechanical at the call sites; the label reads inside stay
  as they are, reached through the bundle.
- *Scope of the widening — open.* `KExpression::write_summary` and
  `WorkingExpression::write_summary` are peers, and only the working one holds
  `Spliced` parts. Either widen both to keep the pair uniform, or widen only
  `WorkingExpression` and leave the AST view on the interner it actually needs.
  Recommended: widen both — a reader comparing the two peers should not have to
  work out why their parameters differ.

## Dependencies

**Requires:** none — the renderer and the carried value are both already in
place; this is plumbing.

**Unblocks:** none.
