# Metaprogramming

> **Stale as work, kept as requirements.** Written against the old runtime,
> now behind `pending_rewrite`; the items below record what the language needs
> and are retired by the [rewrite](../rewrite/README.md) as it meets them.

Runtime code assembly for koan: quoted expression values combined by ordinary
functions and spliced with `EVAL`, per
[old_design/metaprogramming.md](../../old_design/metaprogramming.md). The project buys
the language a macro-free metaprogramming story — declarations (`FN`, `OP`,
`GROUP` members) can be assembled as data and installed by splicing, with one
scheduling rule (the EVAL barrier) keeping concurrent blocks deterministic,
and the declaration surfaces' literal spellings (`#(…)` and `(…)`) unified
behind kind-blind readers.

## Items

Every requirements doc in this retired project.

- [Declaration windows gate dispatch resolution](declaration-windows-gate-dispatch.md)
- [EVAL splices in place](eval-splices-in-place.md)
- [Group members may arrive by splice](group-members-by-splice.md)
- [One kind-blind reader per shape slot](one-reader-per-shape-slot.md)
- [Parse at runtime](parse-at-runtime.md)
