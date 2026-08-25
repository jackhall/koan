# Deferred signature summaries

**Problem.** Registration pays for a diagnostic that almost never fires.
`OverloadSeal::of_delivered`
([src/machine/core/carrier_witness.rs](../../src/machine/core/carrier_witness.rs))
renders every registered callable's signature through `KFunction::summarize`
([src/machine/core/kfunction.rs](../../src/machine/core/kfunction.rs)) — a `Vec<String>`
of elements joined into a `format!`ed `String` — at seal time, and the bucket entry
bump-copies it, all so the `DuplicateOverload` diagnostic can name the colliding overload
without re-opening the sealed carrier
([src/machine/core/bindings.rs](../../src/machine/core/bindings.rs), `write_overload`).
Builtin seeding pays this for every builtin: ≈1,190 of the empty program's 2,874 startup
allocations (dhat, 2026-08-18), and every user `FN`/`OP` registration pays it again.

**Acceptance criteria.**

- Registering a callable renders no signature text: the seal-time `summarize` call and
  the bucket's bump-copied summary are gone.
- A `DuplicateOverload` error names the colliding overload by its dispatch identity —
  keywords and slot types rendered from the stored dispatch token, `fn(DOUBLE :Number)` —
  with no argument names in the text.
- The recorded empty-program startup baseline in [audit/README.md](../../audit/README.md)
  drops by the deferred share and is re-measured to the new figure.

**Directions.**

- *Collision-time render source — decided.* Render from the bucket entry's stored
  dispatch token on the error arm: each keyword's `KeywordSymbol` resolves back to text
  through the run's interner, and each slot renders as its `KType::name`. The "no write
  verb ever opens a carrier" seal discipline holds even on the collision arm, and the
  token stays unmodified — pure dedupe identity, no name freight, no bespoke equality.
- *Rendered form — decided.* The diagnostic drops argument names: overloads collide on
  keywords and slot types (`indistinguishable_from` is name-independent), so the text is
  exactly the collision's evidence — `fn(DOUBLE :Number)`, slot types in the same
  `:`-sigil convention `render_param_record` uses. Argument names remain the *by-name*
  identity: a function value's `value_ktype` keys its parameter record by declared name
  and renders through `KType::name`, and by-name call paths are untouched here.

## Dependencies

**Requires:** none.

**Unblocks:** none.
