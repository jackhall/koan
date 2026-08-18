# Argument currency keyed by borrowed name

**Problem.** Passing arguments into a call clones a `String` per parameter, per call.
`map_arg_carriers` ([src/machine/execute/decide/exec.rs](../../src/machine/execute/decide/exec.rs))
builds a `Record<&DeliveredCarried>` by `record.insert(arg.name.to_string(), carrier)`,
and `arg.name` is already a bumped `&'a str` living in the region — the clone copies text
that is guaranteed to outlive the call. `Record<V>`
([src/machine/model/types/record.rs](../../src/machine/model/types/record.rs)) is an
`IndexMap<String, V>`, so each call also pays the map's own allocation. The builtin lane
pays the pair twice: once for the carriers and again for the `Record<Held>` that
`bind_args` returns.

`Record<V>` is koan's *record value* type, used in ~74 places, and its keys are genuinely
owned where a record is a value. The waste is confined to the two transient
argument-currency uses, which are call-local and never become a koan value.

**Acceptance criteria.**

- Re-keying slot-indexed argument carriers onto their parameter names allocates no
  `String`: the parameter name is carried as the borrowed `&str` the signature already holds.
- The user-defined-call lane's argument values reach the frame bind through the same
  borrowed-name currency, so a call pays one container at most, not two.
- `Record<V>` keeps owned `String` keys where it is a koan record *value*; no call site
  that stores a record as a value is changed.
- The recorded per-call allocation count for a builtin call with n parameters drops by
  the 2n names plus the second container.

**Directions.**

- *Currency shape — open.* A `(&'a str, V)` slice, sized off the signature's element
  count, versus a `Record`-shaped type parameterized over its key. Recommended: the
  slice — argument arity is small, the readers walk it in declaration order, and it
  keeps `Record<V>` untouched.
- *Where the slice lives — deferred.* Whether it is a stack array, an owned `Vec`, or a
  scratch-arena buffer is decided once
  [the step-scoped scratch arena](step-scratch-arena.md) exists; the borrowed keying is
  the win either way and does not wait on it.

## Dependencies

**Requires:** none.

**Unblocks:** none.
