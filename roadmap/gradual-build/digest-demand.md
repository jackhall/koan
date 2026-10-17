# Where a content digest is demanded

The load places every content-digest demand the run makes.

**Problem.** A content digest is computed on demand and kept for one demand
(`src/values/digest.rs`), and the run demands one in two places the load
can name. A call of a keyed body digests the argument at each keyed
parameter (`src/program/specialize.rs`); a module's birth digests each
capture for its content (`src/knot/module/birth.rs`), whether or not a view
or a key ever asks for it. So a keyed call over a list digests the list at
the seek and again at the module the callee births over it, and a module
born each hop over a value that grows with the loop — `MODULE m OVER #[link]`
over a chain of `{next :Any}` records — hashes the rest of the chain each
hop: quadratic in the loop's length. Measured over the caching suite's
programs (`scratch/eviction-and-handle-bits.md`): one chain walk per hop in
`wrapped_peeks`, three in `HELPER_LOOP`, with 2–5 demands per hop, while the
seven programs that pass a received module bare visit one part per demand.
Nothing in the suite watches digest visits, so the growth is invisible to the
flatness tests. Which parameters key is decided by variance alone
([keys](../../src/build/README.md#keys-which-bindings-make-a-specialization)),
so a parameter of list type keys and is digested though its content is
run-built and its digest costs its size.

**Acceptance criteria.**

- A parameter keys only where its static type is a signature or carrier
  type — content whose digest is one stored word; a parameter of any other
  type keys nothing, and the site over it routes cold.
- The digests a seek computes are written into the callee's keyed parameter
  slots at the frame's birth, and a slot the load marks as digested keeps a
  digest memo beside its value, filled at first demand: a program calling
  `TAKE l` four times over one list digests `l` once.
- A module's content is computed where a `:|` or a key first reaches the
  module, read off the view tree at the load, and kept on the module; a
  module surfaced by `USING … SCOPE`, read with `.` or passed on digests
  nothing. A test counts digest visits per hop over the caching suite's
  programs and asserts each is flat in depth or names the module it views
  per hop as the one that pays.
- The digest-visit and digest-demand counters of the measurement ship under
  the `counters` feature.

**Directions.**

- *A digest memo on every composite header and knot root* — deferred. It
  would make a chain's digest linear over a loop that views a module built
  over the chain each hop, at 16 B per header on every value; held until a
  program shows that shape outside a test.

## Dependencies

What a key covers is the build module's
([keys](../../src/build/README.md#keys-which-bindings-make-a-specialization));
what a module's content is, the module design's
([a module is its content](../../design/modules.md#a-module-is-its-content)).

**Requires:** none.

**Unblocks:** none.
