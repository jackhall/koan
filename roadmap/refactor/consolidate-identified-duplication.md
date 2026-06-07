# Consolidate identified code duplication

A targeted pass over six concrete, already-located duplication clusters — collapsing
each to a single owner. Unlike the broad
[naming-and-responsibility audit](naming-and-responsibility-audit.md), the candidates
here are already surfaced with file:line evidence; this item is the execution.

**Problem.** Six clusters of copy-paste logic exist today, each with no single owner:

- *`binder_name` reimplemented per builtin.* Eight files carry a
  `pub(crate) fn binder_name(expr: &KExpression<'_>) -> Option<String>`; five are
  byte-identical one-liners delegating to `expr.binder_name_from_type_part()`
  ([sig_def.rs:85](../../src/builtins/sig_def.rs),
  [module_def.rs:116](../../src/builtins/module_def.rs),
  [union.rs:155](../../src/builtins/union.rs),
  [recursive_types.rs:200](../../src/builtins/recursive_types.rs),
  [newtype_def.rs:320](../../src/builtins/newtype_def.rs)). The remaining three
  ([let_binding.rs:207](../../src/builtins/let_binding.rs),
  [val_decl.rs:175](../../src/builtins/val_decl.rs),
  [fn_def/signature.rs:180](../../src/builtins/fn_def/signature.rs)) extract differently.
- *FN / FUNCTOR body duplication.* [`functor_def::body`](../../src/builtins/functor_def.rs)
  re-implements the slot-extraction, param-name collection, and elaboration sequence of
  [`fn_def`](../../src/builtins/fn_def.rs), plus a parallel
  `functor_def::collect_param_types` for its verdict system.
- *Scheduler `Object` / `Type` finalize arms.* In
  [`scheduler/execute.rs`](../../src/machine/execute/scheduler/execute.rs) the
  `Carried::Object` arm and the `Carried::Type` arm run near-identical declared-return
  extraction, `matches_value` mismatch check, and re-tag — the `Type` arm's own comment
  names itself "the type-channel analog of the `Object` arm above."
- *`finish.rs` `run_combine` / `run_catch`.*
  [`run_combine`](../../src/machine/execute/scheduler/finish.rs) and `run_catch` share
  identical `BodyResult::{Value, Tail, DeferTo, Err}` arms, differing only in the finish
  condition.
- *`dict_literal` `accept_colon` / `accept_equals`.*
  [`accept_colon`](../../src/parse/dict_literal.rs) and `accept_equals` are ~80% identical
  state machines differing only in mode and error strings.
- *Slot-extraction error envelope.* The
  `match extract_kexpression(...) { Some(e) => e, None => return err(...) }` envelope recurs
  ~25 times across ten builtin files, and the `"<BUILTIN> <slot> slot must be a parenthesized
  expression"` error text recurs across ten files
  ([grep `must be a parenthesized expression`](../../src/builtins)) — no shared constructor.

**Acceptance criteria.**

- A single shared `binder_name` (or direct `binder_name_from_type_part` call) replaces the
  five identical delegating copies; the three divergent extractors are either unified or
  documented as intentionally distinct.
- FN and FUNCTOR share one slot-extraction-and-elaboration path; the FUNCTOR-specific verdict
  logic is the only FUNCTOR-only code remaining.
- The scheduler's declared-return check exists once, parameterized over the lifted carrier,
  not duplicated across the `Object` and `Type` arms.
- `run_combine` and `run_catch` share their common `BodyResult` arms through one helper.
- `accept_colon` and `accept_equals` share one parameterized implementation.
- The slot-extract-or-error envelope and the parenthesized-slot error text each have one
  constructor that builtins call.

**Directions.**

- *Per-cluster independence — decided.* Each cluster is a self-contained edit landable on its
  own; the item is a checklist, not one atomic rewrite.
- *Scheduler-arm consolidation vs. carrier unification — open.* The `Object` / `Type` arm
  duplication is the surface symptom of the `Carried::Type` / `Carried::Object` fork that
  [type values as data carriers](../type_language/type-values-as-data-carriers.md) removes.
  Whether to consolidate the arms now or let that item dissolve the fork. Recommended: defer
  this one cluster behind the carrier work; the other five are independent.
- *Error-envelope shape — open.* Whether the slot-extract-or-error envelope becomes a macro,
  a helper on the argument bundle, or a method returning `Result`. Recommended: a bundle
  helper over a macro, keeping the control flow explicit.
- *Execution and validation — decided.* Rewrites go through the `rust-refactor` skill with
  `cargo build` / `cargo test` as the gate; any symbol named in a `//` comment or design doc
  is swept with the documentation skill's `doclinks` pass.

## Dependencies

Overlaps the [naming-and-responsibility audit](naming-and-responsibility-audit.md), whose
"duplicated responsibility" category would surface these same clusters; this item is the
pre-located subset ready to execute without the full sweep.

**Requires:** none — foundation.

**Unblocks:** none tracked yet.
