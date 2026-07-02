# Shrink witnessed.rs's raw retype surface

**Problem.** `Erased::reattach` (`src/witnessed.rs:155-158`) is the named,
contract-documented lifetime re-anchor, yet three accessors open-code the same
`unsafe { retype::<T::At<'static>, T::At<'_>>(…) }` with their own multi-line
SAFETY comments restating the identical argument: `map` (:444), `merge` (:527
and :528), and the sealed read at :580 — while `SealedExtern::open` already
routes through `reattach`. Separately, the ~10-line `Cart` / `RefFamily`
doctest fixture is pasted into five `compile_fail` soundness guards (:319-341,
:364-387, :409-434, :468-501, :649-672, with a variant at :784-800); any
signature change to `Witness` / `WitnessRegion` / `Reattachable` fans out to
each hand-synced copy.

**Acceptance criteria.**

- `map`, `merge`, and the sealed read route their re-anchor through
  `Erased::reattach`; direct `retype` callers in witnessed.rs are exactly the
  documented wrappers (`erase_to_static`, `with_branded_ref`, `reattach`).
- The compile_fail doctest fixture is defined once (e.g. a `#[doc(hidden)]`
  module the guards import) and every guard still fails to compile for the
  same reason it does today.
- No public API change; existing tests and doctests green.

**Directions.**

- *Route, don't relabel — decided* per the repo's compile-enforcement
  preference: the fix is fewer raw `unsafe` sites behind one audited wrapper,
  not copied SAFETY comments.
- *Fixture home — open.* `#[doc(hidden)] pub mod` vs `include!`-style shared
  preamble. Recommended: the hidden module — doctests import it like any
  dependency.

## Dependencies

Shrinks the audited `unsafe` surface the extracted library will export; the
north star is [design/scheduler-library.md](../../design/scheduler-library.md).

**Requires:** none — leaf hygiene.

**Unblocks:** none tracked.
