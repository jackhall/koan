# One producer-disposition primitive and the `Deps` builder

Layer 0 and 1 of the consumer API in
[design/scheduler-library.md](../../design/scheduler-library.md): the
park-classification primitive and the dep-list currency.

**Problem.** Two scheduler conventions are re-implemented by hand at every
consumer site.

*Disposition.* Before parking on a still-finalizing producer, a consumer must
check, in order: is the producer's result ready; if ready, did it error
(propagate a clone of the error); would the park edge create a cycle
(`SchedulerDeadlock`); otherwise park. This ladder is written once for the
`NameOutcome` currency (`disposition_for_producer`,
`src/machine/execute/dispatch.rs:123-139`) and re-derived inline at four
`Outcome`-currency sites: `dispatch/keyworded.rs:201-217`
(`install_overload_park`), `dispatch/single_poll.rs:101-127` (`bare_type_leaf`),
`dispatch/single_poll.rs:277-287` (`type_call`), and
`dispatch/fn_value.rs:36-49`. The sites agree on the ladder and diverge only in
what they do on ready-Ok. Nothing enforces that a new park site performs the
checks at all.

*Dep-list layout.* A `NodeWork`'s dep vec is `[park_producers..., owned_subs...]`
with a `park_count` prefix length (`src/scheduler/nodes.rs:12-14`).
`NodeWork::new` accepts any `(deps, park_count)` unvalidated, and machine code
re-derives the layout arithmetic by hand: `terminals[park_count + j]`
(`dispatch/literal.rs:58, :101`), `results[park_count..]`
(`dispatch/field_list.rs:91, :153`), `if i < park_count`
(`execute/runtime.rs:548`), `deps[park_count..]` (`execute/run_loop.rs:150`).
The park-producer dedup + cycle check is additionally duplicated inside
`keyworded.rs`'s `part_walk` (:330-344 vs :371-381).

**Acceptance criteria.**

- The scheduler surface exposes one producer-disposition classification
  (working name `producer_disposition(producer, consumer)` per the design doc)
  returning errored / ready / would-cycle / park; `disposition_for_producer`
  and the four dispatch sites route through it, each keeping only its own
  ready-Ok policy.
- A `Deps` builder (scheduler-side, naming no Koan type) is the only way
  production code assembles a `NodeWork` dep list: park edges via a dedup'ing
  `park_on`, owned deps via methods on the builder.
- The `[park..., owned...]` layout arithmetic exists only inside the scheduler;
  machine code addresses dep results through accessors, and the five
  hand-slicing sites above are gone.
- The parked-producer handling in `keyworded.rs` (cycle check → deadlock error
  → dedup push) appears once.
- No new scheduler API names a Koan type. Existing tests green.

**Directions.**

- *Disposition returns classification, callers keep policy — decided* per
  [design/scheduler-library.md](../../design/scheduler-library.md) § The
  consumer API: the ready-Ok policies genuinely diverge per lane, so the
  primitive classifies and the caller acts.
- *Builder home — decided:* `src/scheduler/` (the library owns the consumer
  API per the design doc's boundary); dispatch imports it.
- *Result addressing — open.* (a) a `DepIndex` newtype handed back by the
  builder and resolved against the results slice; (b) accessor methods on the
  resolved-deps view (`park_producers()`, `owned(i)`). Recommended: (b) —
  keeps finishes free of index bookkeeping.
- *`NodeWork` internal storage — open.* Two vecs vs validated concat + count.
  Either satisfies the criteria; pick whichever leaves the scheduler's
  free/alias walks untouched.

## Dependencies

Foundation of the [scheduler_library](README.md) project; the north star is
[design/scheduler-library.md](../../design/scheduler-library.md).

**Requires:** none — foundation.

**Unblocks:**

- [The `Await` envelope builder](await-envelope-builder.md)
