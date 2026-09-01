# Module scope consolidation

Teach the environment copy
([lazy-closures.md § Lazy close](../../design/lazy-closures.md#lazy-close-the-copy-verb-through-callables))
to rebuild a module's environment, so an escaping module value — and the
`USING … SCOPE` window that aliases one — stops pinning its producer chain.

**Problem.** A **module value** never consolidates. `consolidate_object`
([scope/copy.rs](../../src/machine/core/scope/copy.rs)) takes only a
`KFunction`: a module's environment *is* its child scope rather than a captured
chain hanging off a callable, and rebuilding the value around a copy of that
scope is a surface the engine does not open. So the value rides verbatim under
the pin and holds the body's region however small and closed the body is.

That pin is not conditional on anyone closing over the body. A `MODULE`
declared in a producer frame pins its body region through the **relocated module
value binding** alone: measured at producer depths 0/1/3, every module-shaped
escape holds 2/3/5 regions, against the 1/1/1 a bare `OP` in the same frame
answers. Since the copy engine re-births a `GROUP` body's own scope kind and
operator registry, that binding is the sole residual pin in an escaping
group-body closure — the census twin
`a_group_body_on_the_captured_chain_retains_nothing_extra`
([close_over/tests.rs](../../src/builtins/close_over/tests.rs)) can therefore
only assert equality against another module-shaped escape, not the `(1, 0)` an
operator-free chain answers.

A captured chain holding a **`USING … SCOPE` window** pins for a second reason
that is really the same one. The window scope's bindings are `Borrowed` — a
read-only façade over the opened module's own table, living in *that* module's
region — so `Scope::is_copy_ready`
([scope.rs](../../src/machine/core/scope.rs)) declines it on the
owns-its-bindings clause, though the module scope it aliases may be closed and
cheap. A window is an alias of a module scope, so rebuilding one and
consolidating the other are one surface, not two.

What the readiness gate already models is a `Module` body announcing nothing,
group record and all. What it still declines is a body carrying an
[`AnnouncedWindow`](../../src/machine/core/scope.rs) — the mutually-visible
`NEWTYPE` / `UNION` declarations a pre-scan found — which nothing rebuilds.

**Acceptance criteria.**

- An escaping module value whose body scope is closed and claim-free
  consolidates on the same terms a closure does, and the copy releases the
  body's producer region.
- A closure escaping a producer chain that declared a `MODULE` in the frame
  holds **one** region and releases everything else at every producer depth,
  where the census today reads depth + 2.
- A captured chain holding a `USING … SCOPE` window consolidates rather than
  pinning, and the window's copy and the module scope it aliases resolve the
  same members without copying that table twice.
- A `MODULE` body carrying an announced declaration window passes the readiness
  gate, and a `NEWTYPE` declared in a copied body resolves in it exactly as it
  does in the source.

**Directions.**

- *Rebuilding the window — open.* Re-birth the `AnnouncedWindow` at the
  destination the way the group record is re-birthed (its runs are bumped into
  the body's own region and the record is `Drop`-free), or keep the gate's
  decline and consolidate a module value by rebuilding only what its members
  reach.
- *`Borrowed` window scopes — open.* Copy the façade through the module scope's
  memo entry — a window and its module value share one copy, which is the
  reading that makes this one surface — or give the window scope an owned
  copy of the members it surfaces.
- *Where the module value is rebuilt — open.* Widen `consolidate_object` to the
  `Module` arm, so the value takes the same relocation fold a callable does, or
  leave the verb callable-only and sever at the module *binding* install
  ([`adopt_binding_pinned`](../../src/machine/core/scope/registry.rs)), which is
  the one door that pins a module today.

## Dependencies

The sibling item [Callable copy tuning](callable-copy-tuning.md) owns the
pricing decisions taken at a crossing; this item is a capability the pricing has
nothing to price yet.

**Requires:** none — the copy engine it extends, including a windowless
`MODULE` body's group record, is shipped.

**Unblocks:** none tracked yet.
