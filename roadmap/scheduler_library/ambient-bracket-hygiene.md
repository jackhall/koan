# Ambient bracket hygiene

**Problem.** The per-step ambient state on `KoanRuntime`
(`src/machine/execute/ambient.rs`) is bracketed by hand, with no Drop
backing anywhere. `enter_slot_step` returns a `SlotStepGuard` that is a
plain save-struct the caller must remember to pass back to
`exit_slot_step` — a path that early-returns or unwinds between the two
leaves the node's values installed and the saved previous ones never
restored. `swap_active_frame` is a raw `mem::replace` whose paired restore
is a second manual call at each site (`dispatch_body`'s body-sub-slot
bracket). `active_in_contract_chain` is a bare
`pub(in crate::machine::execute)` field written directly per step in
`KoanRuntime::execute`. Nothing ties an install to its restore.

**Acceptance criteria.**

- Every per-step ambient install/restore pair is a bracket by construction
  — closure-scoped or Drop-backed — so no return path, including unwinds,
  exits a step with the saved ambient values unrestored.
- `swap_active_frame`'s call sites go through the same bracket device; no
  manual save/restore `mem::replace` pair over ambient state remains
  outside it.
- `active_in_contract_chain` is set and cleared only through the bracket,
  not by raw field writes at step sites.
- Step semantics are unchanged — the `PostStep` frames the Replace arm
  reads are still produced, and existing scheduler / TCO tests stay green.

**Directions.**

- *Bracket device — open.* (a) Closure-scoped —
  `with_slot_step(frame, reserve, payload, |rt| …)` returning the step
  result alongside `PostStep`; (b) a Drop-backed guard borrowing
  `&mut KoanRuntime`. `exit_slot_step` returns data (`PostStep`) that the
  Replace arm consumes, which a pure Drop guard cannot hand back.
  Recommended: (a), with Drop kept only as the unwind backstop if one is
  needed.

## Dependencies

**Requires:** none — leaf hygiene on the driver side.

**Unblocks:** none tracked.
