# Migrate `vend_carrier` sites onto `Sealed`

Move the continuation and contract carriers off the loose `vend_carrier` function and onto the
`Sealed` access surface, deleting the wrapper.

**Problem.** The scheduler's continuation and contract carriers are stored `Erased` on the
lifetime-free node and re-anchored through [`vend_carrier`](../../src/witnessed.rs) — ~17 call
sites across `run_loop.rs` / `finalize.rs`. With `attach` reimplementing `vend_carrier`, these
sites route a loose function rather than the `Sealed` method, so the wrapper persists as a second
spelling of the same primitive.

**Acceptance criteria.**

- The ~17 `vend_carrier` call sites read their continuation / contract carrier through
  `Sealed::open` (copy-out where the value does not escape) or `Sealed::attach` (only where a
  reference must ride up-stack); the `vend_carrier` function is deleted.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Prefer `open`, reach for `attach` — decided.* Each site uses `open` + copy-out unless a
  reference genuinely escapes the access, so the later [remove-attach](remove-attach.md) item has
  the smallest possible residue.

## Dependencies

**Requires:**

- [Externally-witnessed sealed form and `attach`](externally-witnessed-attach.md) — supplies the
  `Sealed` access methods these sites move onto.

**Unblocks:**

- [Remove `attach`](remove-attach.md) — one of the four carrier/read migrations that must land
  before `attach` can be deleted.
