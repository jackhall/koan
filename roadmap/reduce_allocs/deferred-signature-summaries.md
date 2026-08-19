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
- A `DuplicateOverload` error still names the colliding overload's signature in the same
  rendered form as today.
- The recorded empty-program startup baseline in [audit/README.md](../../audit/README.md)
  drops by the deferred share and is re-measured to the new figure.

**Directions.**

- *Collision-time render source — open.* Where the diagnostic's text comes from once
  nothing is pre-rendered: render from the bucket entry's stored dispatch token (the
  `StoredDispatchTokenElement` run already encodes keywords and argument shape) versus
  opening the colliding sibling's `SealedFunction` on the error path to call `summarize`
  there. Recommended: the stored token, keeping the "no write verb ever opens a carrier"
  seal discipline intact even on the collision arm.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Deferred return-obligation labels](deferred-return-obligation-labels.md) — shares the
  error-time callable render this item settles.
