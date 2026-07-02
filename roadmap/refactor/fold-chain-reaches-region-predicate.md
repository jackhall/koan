# Own the chain-reaches-region predicate

**Problem.** "Does holding this scope / cart keep region X alive" is implemented three
times. The omission closures in `Scope::fold_reach` (`src/machine/core/scope.rs:217-221`)
and `Scope::foreign_reach_of` (`scope.rs:815-820`) are verbatim twins —
`home.pins_region(region) || self.ancestors().any(|s| ptr::eq(s.region(), region))` —
and `cart_chain_reaches_region` (`src/machine/execute/runtime/submit.rs:28-38`)
re-implements the lexical-ancestor half as a standalone loop. The two scope.rs folds
(`fold_reach` accumulating into the scope's reach-set, `foreign_reach_of` returning a
fresh set) differ only in where the result lands. Adjacent micro-fold:
`FrameSet::fold_foreign` (`src/machine/core/arena.rs:537`) is expressible as
`fold_foreign_omitting(other, |r| home.pins_region(r))`.

**Acceptance criteria.**

- The lexical-ancestor region walk is one named `Scope` method; `fold_reach`,
  `foreign_reach_of`, and `resolve_node_scope`'s cart check route it, and
  `runtime/submit.rs` carries no standalone chain-walk region predicate.
- The shared omission predicate (home pins ∪ lexical ancestors) appears once;
  `fold_reach` and `foreign_reach_of` differ only in accumulate-into-self vs
  return-new.
- `FrameSet::fold_foreign` delegates to `fold_foreign_omitting` (one insertion loop in
  arena.rs).
- Reach-set contents and `NodeScope` decisions are unchanged — existing tests green.

**Directions.**

- *Method granularity — open.* (a) One composable lexical-walk method
  (`Scope::chain_reaches_region`) with call sites composing `pins_region` themselves;
  (b) one combined predicate taking `Option<&Rc<FrameStorage>>` for the home. Recommended:
  (a) — `cart_chain_reaches_region` needs only the lexical half, so composition avoids
  an artificial `None` home at that site.

## Dependencies

**Requires:** none — leaf cleanup.

**Unblocks:** none tracked.
