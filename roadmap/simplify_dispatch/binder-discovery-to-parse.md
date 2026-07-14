# Move binder discovery into the parser

Verify the AST recursion that discovers a submission's binders, then compute its
parse-static portion once at parse time instead of re-deriving it on every submission.

**Problem.** Binder discovery runs at dispatch-submission time and is re-derived
every time an expression is submitted. The chokepoint
[`KoanRuntime::submit_expression`](../../src/machine/execute/dispatch/submit.rs)
recurses into a binder's eager Expression-shaped slots and, at each level, calls
[`extract_binder_install`](../../src/machine/execute/dispatch/submit.rs), which walks
the scope's ancestor chain, looks up the function bucket, and asks each overload's
`binder_name` / `binder_bucket` extractor whether this expression introduces a binder
and which slots are eager. A function body re-submits on every call, so this walk
repeats per invocation even though the bulk of it is structural — the keyword leading
the expression, and the name read out of `expr.parts[1]`, are fixed at parse time.
[`KExpression`](../../src/machine/model/ast.rs) already caches a `DispatchShape`,
`untyped_key`, and operator probe at parse time via `fill_cache`; the binder shape is
not cached alongside them.

The recursion's *correctness* is also unspecified: which AST forms introduce a binder
is spread across per-builtin extractors (`binder_name` in `let_binding.rs`,
`val_decl.rs`, `sig_def.rs`, `module_def.rs`, `union.rs`,
`recursive_types.rs`, …) with no single statement of "what introduces a binder" and no
exhaustive test that the recursion installs every nested binder form.

The install channel itself is encoded twice. Submission models the two binder
channels as the mutually-exclusive `enum BinderKey` (submit.rs), while dispatch's
`build_resolved`
([resolve_dispatch.rs](../../src/machine/execute/dispatch/resolve_dispatch.rs)) runs
the *same two extractors* into `Resolved.placeholder` /
`Resolved.pending_overload_bucket` — two independent `Option` fields whose exclusivity
is nowhere enforced — and `keyworded::initial`
([keyworded.rs](../../src/machine/execute/dispatch/keyworded.rs)) installs them again
with the same binding index. Downstream, the binder pre-subs ride a channel almost no
consumer wants: `classify_dispatch`
([dispatch.rs](../../src/machine/execute/dispatch.rs)) threads `pre_subs` through all
twelve `DispatchShape` arms, but only `Keyworded` consumes it, so ten arms open with
`debug_assert!(pre_subs.is_empty())` — the invariant "binder pre-subs exist ⇒ shape is
Keyworded" is held at a distance between submit.rs and dispatch.rs instead of by a
type.

**Acceptance criteria.**

- The set of binder-introducing AST forms is specified in one place and covered by a
  test asserting the recursion installs every nested binder (LET, VAL, FN,
  MODULE, SIG, UNION, NEWTYPE, RECURSIVE TYPES) at submission.
- The parse-static portion of binder discovery — the eager-slot mask and the binder
  name/bucket read out of the AST's structure — is computed once at parse time and
  cached on `KExpression` beside `DispatchShape`, not recomputed per submission.
- Submission reads the cached binder plan; the only work left at submission is the
  genuinely scope-dependent residue (resolving which user FN overloads in
  scope are binder-shaped), if any remains.
- Binder installs flow through one mutually-exclusive `BinderKey`-typed channel from
  extraction to install — submission and `Resolved` share it, and no struct carries the
  placeholder and pending-overload channels as independent `Option` fields.
- `pre_subs` reaches only the `Keyworded` dispatch path: the other `DispatchShape`
  arms neither receive the parameter nor assert on it, and the per-arm
  `debug_assert!(pre_subs.is_empty())` guards are gone because the types make the
  states they checked unrepresentable.
- Behavior is unchanged: the same placeholders and pending-overload entries install at
  the same point in the submission flow.

**Directions.**

- *Verify before relocating — decided.* Land (or confirm) the exhaustive
  nested-binder install test first, so the move is demonstrably behavior-preserving.
- *Scope-static vs scope-dependent split — open.* Builtins are unshadowable and the
  keyword→binder-shape mapping is static, so a builtin-led binder is fully
  parse-determinable; a user FN binder overload depends on scope. Decide
  whether the cache carries a full plan for the builtin case plus a scope-resolved
  residue, or only the structural eager-slot mask. Recommended: cache the structural
  eager-slot mask and the extracted binder name/bucket on `KExpression`; keep the
  overload-bucket lookup at submission.
- *Cache home — open.* Fold the binder shape into `KExpression::fill_cache` alongside
  the existing parse-time caches. Recommended.
- *`BinderKey` home and `pre_subs` routing — open.* Where the shared `BinderKey` enum
  lives once both submit.rs and `Resolved` use it, and how `pre_subs` is confined to
  the keyworded path: (a) `classify_dispatch` hands `pre_subs` straight to the
  keyworded constructor and the other eleven arms never see the parameter; (b) the
  dispatch work-item type carries `pre_subs` inside its `Keyworded` variant.
  Recommended: (a) — it is the smaller signature change and already deletes every
  guard.

## Dependencies

An engine-internal parse/dispatch-path hygiene item. Update
[design/expressions-and-parsing.md](../../design/expressions-and-parsing.md) (the
structural parse cache) and [design/execution/README.md](../../design/execution/README.md)
(submission-time binder install) if the vocabulary they name changes.

**Requires:** none — engine-internal.

**Unblocks:** none tracked yet.
