# A hold no binding can fit, refused at load

A `:!` over a module binder at a signature no instance of the binder's own
signature can fit, refused where it is written.

**Problem.** A member read or a call over a module binder that no instance of
its declared signature can satisfy is refused at load, called or not: under
`peek = (FN :{m :Counter} -> Any = #(m.extra))`, `peek` is refused uncalled
([`signature_outside`](../../src/scope/heads.rs)). A hold is not:

```koan
SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
SIG Extra = #[(VAL extra :Str)]
LET peek = (FN :{m :Counter} -> Any = #(m :! Extra))
PRINT "start"
```

prints `start`, and `peek` is refused only where a call binds `m`, though no
module `m` may hold declares `extra`.

**Acceptance criteria.**

- `peek` above is refused at load, uncalled, naming `Extra`'s member
  `Counter` lacks.
- A hold at a signature some instance of the binder's signature can fit loads
  and is checked by the call, as today.

**Directions.**

- *Where the check sits — open.* In the hold's own never-satisfies check,
  through `signature_outside` as a member read's check reads it, or as a
  question its binding answers.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
