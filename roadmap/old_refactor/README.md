# Refactor

> **Stale as work, kept as requirements.** Written against the old runtime,
> now behind `pending_rewrite`; the items below record what the language needs
> and are retired by the [rewrite](../rewrite/README.md) as it meets them.

Cross-cutting cleanups that keep the engine legible and fast as it grows —
reconciling names with behavior, merging responsibilities that have drifted apart,
and shrinking the unsafe surface. Allocation-cutting work lives in its own project,
[reduce_allocs/](../old_reduce_allocs/README.md).

## Items

Every requirements doc in this retired project.

- [Source the free-identifier walk's last two rules](free-identifier-walk-sourcing.md)
- [One declaration-window representation](one-declaration-window.md)
- [One recognizer for a malformed keyword spine](one-malformed-spine-recognizer.md)
- [Round-trip the builtin forms under arbitrary layout](round-trip-builtin-forms.md)
- [Rebuild the scope-handles verification list](scope-handles-verification-audit.md)
- [Seal as a value-bearing producer](seal-as-producer.md)
- [Substitute, then ask](substitution-walk-collapse.md)
- [Integrate the type lattice](type-lattice-integration.md)
- [One structural walk over `TypeNode`](type-structure-combinator.md)
