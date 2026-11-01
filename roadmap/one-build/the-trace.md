# The trace

Per call site, the binding the load can name.

**Problem.** A body's facts wait on a binding only the run supplies, so a
call keyed on load content seeks a settlement at run
([keys](../../src/build/README.md#keys-which-bindings-make-a-settlement)).
Yet most such bindings are traceable from the text: a functor applied at the
top level to a `LET`-bound module, a `FOR ALL` group solved at a keyworded
call the load selected, a callee reached through a knot's edges or a closed
content tree. The tail facts already run a greatest fixpoint over the call
graph of the bodies whose names do not escape
([load-time types](../../src/scope/README.md#load-time-types)), and keys
already leave out a parameter every tail edge passes on bare, but no fold
says what content a call site passes at a keyed name.

**Acceptance criteria.**

- The tails fold extends from "passed on bare" to "passed this content": per
  keyed parameter of a non-escaping body, the set of load contents its call
  sites pass, transitively through the callers' own parameters. A body
  reachable through a value the load cannot follow is untraced at every
  keyed name.
- Each call site of a keyed body carries on its record the traced binding —
  the content at each keyed name — or is marked untraced; the folds lay it,
  so the run reads no type to find it.
- A count over the example programs: of the settlements a run keys
  ([one layer](one-layer.md)), how many the trace names, by call site. The
  number is reported, not gated; it sizes what
  [traced layers at load](traced-layers-at-load.md) lays.
- The fold is linear in the call sites it reads, as the tails fold is.

**Directions.**

- *What the trace follows — decided.* Load content alone: a type whose handle
  rests in the root, a type value naming one, or a module whose content tree
  is closed under the root — exactly what keys a settlement today. Content a
  composition names is the composition's own and traces nothing.
- *Where it stops — open.* A call site in a body whose name escapes, since
  the body can be called from a site the load cannot read; a call through a
  header or a barrier, whose solve needs the frame's retype; a call site
  passing a parameter bound at more than one content, which traces to a set.
  Whether a set per site is laid in full or capped, and the cap, is sized by
  the count above.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Traced layers at load](traced-layers-at-load.md) — the bindings the load
  lays layers under.
- [Invariance around a tail cycle](cycle-invariance.md) — refines the fold's
  "unchanged" once the trace has extended it.
