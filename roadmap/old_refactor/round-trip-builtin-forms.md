# Round-trip the builtin forms under arbitrary layout

The parser's round-trip law covers every builtin spine at every admissible layout, not just the
canonical spelling.

**Problem.** The round-trip law — `describe(parse(render(tree, tape))) == expected(tree)` —
runs over a generated tree whose keyword pool (`ZZ QQ WW + * < > |`,
[properties.rs](../../src/parse/tests/properties.rs)) is deliberately disjoint from the
keywords [`FORMS`](../../src/parse/forms.rs) spells. No generated run therefore matches a
`FORMS` key, and three things go untested under arbitrary layout as a result: the parse-time
bare-parenthesized type-slot rewrite
([`admit_bare_type_slots`](../../src/parse/forms/binder.rs)) never fires inside the law, since it
only rewrites at a masked slot of a matched key; a form's lazy stamp and binder plan are never
observed on a run the tape spelled across indented child lines, redundant `(…)` wrappers and
sprinkled commas; and the `:(…)` ≡ `(…)` equivalence at those slots is pinned only by the three
surface tests that survived in [type_sigil.rs](../../src/parse/tests/type_sigil.rs).

What does cover the builtin spines is narrower. Law 24
([forms/tests/binder.rs](../../src/parse/forms/tests/binder.rs)) renders each `FORMS` entry with
fresh identifier fillers at one canonical layout and checks the cache it produces; the
table⟺registration law ([registration.rs](../../src/parse/forms/tests/registration.rs)) compares
keys to live signatures and parses nothing. Every other reading of a builtin spine under a
non-canonical layout is incidental, from an end-to-end test that happened to be written with an
indented body.

**Acceptance criteria.**

- The generator draws keywords from `FORMS`'s own spellings as well as from the disjoint pool, so a
  generated run matches a builtin key.
- The oracle models the bare-parenthesized type-slot rewrite, so a rendered `(…)` at a masked slot
  is expected to read back as `SigiledTypeExpr`.
- The round-trip law holds over builtin forms under every layout the tape spells — indented child
  lines, redundant wrappers, ignorable commas — and the type-slot flip is asserted inside it rather
  than by a separate pin.
- A form's cached entry, its binder plan and its lazy stamp are asserted on a run rendered at a
  tape-chosen layout, not only at the canonical spelling.

**Directions.**

- *Where the builtin keys enter the generator — open.* Candidates: a second `Tree` arm that draws a
  whole `FORMS` entry and fills its slots recursively, or widening the keyword pool and letting the
  existing arms assemble matching runs by chance. Recommended: the entry arm — a matching run is
  vanishingly unlikely to assemble by chance across an eight-keyword spine.
- *How the oracle learns the rewrite — open.* Candidates: the oracle reads `type_slots` off the same
  `FORMS` entry the generator drew (one source of truth, but the law then cannot catch a wrong mask),
  or the test spells the masked positions independently. Recommended: read the entry, and leave mask
  correctness to the table⟺registration law that already owns it.

## Dependencies

**Requires:** none — the property suite it extends has shipped.

**Unblocks:** none tracked yet.
