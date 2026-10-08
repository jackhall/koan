# One judgment, one function

**Problem.** Several judgments the load makes are made again by the run, or by
a second load pass, through a second implementation that agrees with the first
only by test. Dispatch's tie rule (a lone survivor, else a builtin among
several, else ambiguous) is written in [`statics.rs`](../../src/dispatch/statics.rs)
and again in [`select.rs`](../../src/dispatch/select.rs), and `outranked`
restates `select_by_class`'s elimination; only the debug `select::agree` ties
them, and their messages have drifted (one renders with backticks, the other
without, and the load's no-overload message renders an instance argument as
`Any`). `retyped_to` in statics restates `Seen::seen_at`'s target rule
([`values/surface.rs`](../../src/values/surface.rs)) as `TypeNode` patterns,
held to it by the debug net `carried_under_static`, which skips a non-concrete
lower end. "Can never satisfy" (both ends bounded above, then the meet is
`Never`) is written six times in statics. `returns_within` in
[`evaluate.rs`](../../src/dispatch/evaluate.rs) and `select::keeps` both answer
whether a return satisfies its contract, as does the program suites' evaluator;
`rules::outside` restates the judge's lower-end test, and `statics::missing`
re-derives the missing field `rules::needs` already knew. In [the module coercion](../../src/knot/module/coerce.rs),
`crossed` is `coerce` with its `expect` turned into an `Err`, substituting both
sides before `coerce` does it again; [`view.rs`](../../src/knot/module/view.rs)'s
keyworded loop repeats `coerce`'s function arm because `coerce` has no
`ExpressionShape` arm; the nested `SignatureApply` arm re-implements `fitted`.
Signature member lookup is written three ways, `reads::manifest_through`,
`rules::member_of` and `layout::member_index`, and they disagree:
`manifest_through` answers `NoSuchMember` for `:(v.inner.Elem)` through a view
whose `inner` slot is declared at an application, where `LET i = v.inner` then
`i.Elem` reads it. A module's member order is spelled three times — a body's
slots, a `USING` block's parameters and a signature's tables — and a test holds
the three equal. Statics and the run classify "is a view
ascription" by different predicates (both ends are signature types, versus the
value is a module or the operator is `:|`), so the load does not refuse
`1 :| Any`.

**Acceptance criteria.**

- Selection has one implementation: the run's `select` and the load's statics
  call one `winner(survivors, is_builtin)` and one message formatter;
  `select::agree` and `outranked` do not exist, and the load's first-class
  elimination of a *maybe* candidate reads the lattice's `outranks`, as
  `select_by_class` does.
- "Can never satisfy" is one function returning the two bounded ends, called
  from every site that refuses on it.
- The exactness rule is one predicate in `values`, read by statics and by
  `Seen::seen_at`; `carried_under_static` checks both interval ends against
  it; a property test over drawn values and types pins that a value retyped to
  a type lies within it at every retype site (ascription, `LET … :T`, a
  parameter, `EVAL … -> T`, a registration's return).
- One function answers "does the return satisfy its contract" for a call by
  name, a keyworded call and the program suites' evaluator; `returns_within`
  and `keeps` do not exist.
- A builtin rule drops a candidate through the judge's own lower-end test,
  `lower_end_outside`, and names the field or member the argument lacks;
  `rules::outside` and `statics::missing` do not exist.
- `coerce` has an `ExpressionShape` arm; `crossed` does not exist;
  `view::build` calls `coerce`; the nested `SignatureApply` arm calls `fitted`.
- One member lookup serves `reads`, `rules` and `layout`; `:(v.inner.Elem)`
  through a view reads the member `v.inner` then `.Elem` reads, pinned by a
  test.
- A module's signature alone places its members: its tie and a `USING` block
  place each member by name through the one lookup.
- Statics and the run classify a view ascription by one predicate; the load
  refuses a view the view door refuses at every run — `1 :| Any` (no module)
  and `m :| Any` or `m :! (A & B)` (no one signature application) — each
  pinned by a test, and every `:!` of a non-signature type loads as before.
- No debug net or test in the tree exists to hold two implementations of one
  judgment equal.

**Directions.**

- *Where the shared selection lives — decided.* In `dispatch/select.rs`, which
  the statics pass imports; statics owns nothing selection-shaped.
- *Where the exactness predicate lives — decided.* In `values`, beside
  `seen_at`, since the carried type is the value's; `rules.rs` and
  `statics.rs` stop importing each other for `under` and `retyped_to`.
- *The strict class win — decided.* The lattice exposes `outranks(a, b,
  class)`; `select_by_class` and dispatch's first-class elimination both read
  it.
- *The rules' need test — decided.* The lattice exposes the judge's lower-end
  test as `lower_end_outside`, which the judge, the builtin rules and the
  evaluator's carried-type net read.
- *Member lookup — decided.* Layout order and the one lookup (`member_of`,
  generalized) live in a new `elaborate/members.rs`: `elaborate` sits below
  `knot`, so `knot/module/layout.rs` cannot serve `reads`. `layout.rs` keeps
  the readers of module values.
- *Layout order — decided.* The signature is the one layout; the shape
  builder's slot order is scope's own and nothing assumes it matches.
- *The agree nets — decided.* Deleted once the implementations are one; a test
  that pins agreement between two copies becomes a test of the one.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
