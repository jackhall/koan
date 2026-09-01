# Lazy closures

How koan severs a value's captured environment from the regions it was built
in: capture is by reference, retention is by pin, and severance is always a
priced *copy* — explicit at the `CLOSE OVER` surface, implicit at the escape
seam once a fat pin is forming. "Lazy" names the placement of the cost: a
closure's definition is O(1) always; the transitive copy of its environment
happens at the latest moment reach evidence justifies it, or never.

## Capture pins; the region is the granule

A function value captures its definition scope as one reference
([`KFunction.captured`](../src/machine/core/kfunction.rs)); an escaping value
pins the region owning that scope, and pins chain — through
`FrameStorage.outer` and through the pinned region's own binding entries'
pins ([memory-model.md](memory-model.md)). Retention is per *region*, never
per value: pinning "just one binding" keeps its whole frame region. So
minimal-capture-at-definition cannot fix fat-frame retention — only copying
captures out can, and that copy is exactly what the escape seam already
prices for plain data ([value-substrates.md](value-substrates.md)). What the
seam cannot do alone is copy through a callable: functions and modules are
borrow leaves of the copy verb.

## CLOSE OVER: explicit severance

```
CLOSE OVER (capture1 capture2 (HELPER _) ...) (
  <statements>
  <tail statement>
)
```

The form is a builtin like any other
([close_over.rs](../src/builtins/close_over.rs)). Its block runs over a
dedicated per-call-tier region with no `outer` link; the tail value returns
homed there, and holders pin exactly that region plus any callable pins. The
block scope's outer is the innermost eternal-homed scope of the enclosing
chain — builtins and top-level definitions stay visible and contribute no
reach; every per-call binding arrives through capture. Block-local bindings
are invisible outside; only the tail escapes.

Capture kinds:

- **Identifier** (value or type channel): resolved at the form's step
  (parking on an in-flight placeholder via the standard resolve path) and
  relocated under the `Copy` verb — data rebuilt transitively, strings
  re-bumped, type handles copied by value.
- **Signature-shaped pattern** `(HELPER _)` / `(MAP _ USING _)`: names one
  full untyped bucket key — never a bare lead keyword, since dispatch
  registrations are not identifier bindings — and captures that registration
  pinned. `_` is the universal hole token.
- **Implicit close**: at block-scope build time, every dispatch registration
  and module binding in the per-call portion of the enclosing chain is
  copied in, pinned with its full transitive reach. Operator registrations
  count as dispatch registrations, and `USING` window scopes are part of the
  walked chain — their copied entries pin the module's region. The build
  parks on every visible in-flight claim in the chain, so what gets closed
  over never depends on scheduling. This is a build-time act, not call-time
  outward resolution — an escaped closure's body dispatches after its
  ancestors are dead, so nothing may resolve outward later.

A `USING` window's surfaced *registrations* and *modules* close implicitly by
that rule; a surfaced value member is data, so it is named in the capture list
like any other datum.

Flattening a chain into one scope keeps the innermost entry for each name,
token and operator probe alike. For the operator registry that is exact, not
lossy: resolution stops at the first scope holding the probe, so a use site
that could have reached an outer declaration does not exist, and the two never
meet inside the block. Handing both to the registry instead would raise its
one-chaining-mode-per-scope conflict — a rule about what a single scope's own
declarations may say, which shadowing scopes were never subject to — and a run
that reduces outside the block would refuse to compile inside it.

A closure defined inside the block is severed by construction: its captured
chain is the block scope, then the eternal tier.

EVAL is permitted; names resolve against the block scope, the captures, and
the eternal chain, and fail as unbound at that boundary otherwise.

### Nothing the block reads is homed in the caller

Severance removes the link that would keep the caller's frame alive, so every
piece of state the block reads *after* that frame retires must be homed
somewhere the block still names. Two placements follow, and both are
invariants of the form rather than incidental choices:

- **The body's working copies rest in the block's own region.** A block's
  statements arrive as raw AST and are frozen into working form, and the
  block's tail is read back from the block frame's own cart, which holds
  nothing of the caller — so freezing at the call site would leave that read
  dangling, and freezing into the eternal tier, the one other region the
  block scope names, would leave one copy per *evaluation* of the form alive
  for the rest of the run. Homing them in the cart makes both false. The cart
  does not exist yet at the step that decides the tail, so the body crosses
  the install as raw AST
  ([`fresh_cart_tail`](../src/machine/core/kfunction/block_tail.rs)) and the
  reinstalled step — running with the fresh cart as its own scope — freezes
  the working run at that cart's brand. Its cost is one extra scheduler hop
  per evaluation.
- **A block statement's result rests in the block's own region.** A statement
  binds in the block, so its terminal is destined at the block frame's region
  rather than at the consuming slot's cart
  ([harness.rs](../src/machine/execute/harness.rs)). Naming the cart would
  have the caller's region retain the block's while the block retains a
  capture back out of it — a ring neither side frees.

## Lazy close: the copy verb through callables

Deep-copying a function deep-copies the data it closes over. The per-call
portion of the captured scope chain is rebuilt at the destination — data
bindings relocated under `Copy`, nested callables recursed, eternal-homed
scopes referenced verbatim, each copied link taking a **fresh `ScopeId`** —
memoized per source scope, so a recursive FN's scope→function→scope cycle
terminates and sibling closures sharing a defining scope share the copy. The
engine is [scope/copy.rs](../src/machine/core/scope/copy.rs); the verb naming
it is `RegionEscape::Consolidate`, the third verb beside `Copy` and `Pin`
([kobject.rs](../src/machine/model/values/kobject.rs)).

The fresh id is free of consequence because a visibility cutoff is
reader-relative: `LexicalFrame::index_for` gates a scope's bindings only while
a live call-site chain names that `scope_id`, and a copy's id is named by no
chain, so every entry in a copied scope reads as visible to every body that
captured it — exactly what the closed source scope already answered. Copied
entries therefore carry no lexical position of their own.

The copy runs **inside the relocation fold**, at the fold's own brand: a
delivered value re-anchors only at the borrow that opens it, never at a region
lifetime, so a bound callable is not read out and rebuilt but *relocated*,
through a nested `transfer_into` whose destination operand pairs the
destination region's handle with the copied scope the rebuilt callable attaches
under (`RegionScopeFamily`,
[ref_carriers.rs](../src/machine/core/ref_carriers.rs)). Source callable and
copied scope meet at that nested fold's brand. The consequence for the memo is
that it can only carry source *addresses* across brands, which is what it is
keyed by; the consequence for the retention claim is that it stays
product-derived like every other relocation's — a rebuilt callable's one region
borrow is its captured scope, so a copy releases the producer and a declined
rebuild that rode verbatim keeps it, out of one predicate.

The **operator registry** is rebuilt rather than relocated. A group record is
resident in the declaring scope's region, and so are the entry's `address`
identity arm and its bumped `declaration` text — copying the map wholesale
would leave the rebuilt scope pointing into the source. But a record's whole
content is its member symbols and its chaining mode, both lifetime-free plain
data, so the fill reads them under the source envelope's own open and births a
fresh record at the destination ([`Scope::birth_operator_group`](../src/machine/core/scope/reach.rs)),
with no nested transfer of the kind a captured callable needs. The rebuild is
memoized on the source record's address for the length of one scope's fill, so
a `GROUP`'s `2ⁿ−1` powerset entries land on **one** reborn record exactly as
the source's entries share one. `OperatorGroup::alloc` re-sorts and re-dedups
what it is handed, so the reborn record renders a byte-identical
`declaration_key`: the registry upsert's structural arm survives the copy
verbatim — an equal redeclaration is still a silent no-op and a disagreeing
chaining mode is still a conflict — and only its address arm, fresh by
construction, differs. The rebuild is priced like every other channel: each
entry charges its flat size plus its declaration text into the scope's
`copy_cost` memo, and the record's own bytes are charged once per record, so
the chooser pays for what it now copies. This is what lets a `CLOSE` block
consolidate at all, since the capture plan flattens every per-call operator
visible on the enclosing chain into the block scope.

A record has a **second home**, and one birth fills both. A `GROUP` body's scope
*kind* names the record its member `OP` declarations belong to
([`ScopeKind::Module`](../src/machine/core/scope.rs)), read back through
`nearest_group_context`; the body's powerset registry entries seal that same
record. The kind is fixed at allocation, so the copy re-births the record
*before* the copied scope exists — the brand-rooted
[`Scope::birth_operator_group_at`](../src/machine/core/scope/reach.rs), which the
scope-rooted door delegates to — stamps it into the copied kind, and seeds the
fill's per-record memo with it, so the copied kind field and every copied entry
name one reborn record exactly as `alloc_group_child` makes the source's share
one. Carrying the kind across is not cosmetic: name resolution reads it.
`nearest_opaque` stops at a `Module`, so a copied group body stamped `Anonymous`
would let a name leak past a wall the source held, and `nearest_group_context`
answers off the same field, so an `OP` inside the copied body resolves to the
copy's own record rather than to nothing.

### Two surfaces, and the readiness gate

The copy fires at exactly two surfaces: a top-level callable crossing the
**priced escape seam** (`copy_or_pin_callable`, which prices the chain's summed
binding-copy memos against what the pin would retain, under the same α as the
substrate chooser and with the same foreign-crossing pin), and an explicit
**`CLOSE OVER` callable capture**, which consolidates unconditionally — the
form exists to cut exactly that retention. A callable *cell* inside a copied
container rides verbatim under `Copy`.

The pricing fact is a per-scope monotone `copy_cost` memo on `Bindings`, bumped
as each value bind applies from the bound value's already-memoized copy weight.
A closure's definition site therefore pays nothing: the bump happens where a
bind was already happening, and the seam's read is O(chain depth) over stored
counters.

A scope is rebuildable only when it is **closed**, owns its bindings (a
`USING … SCOPE` window is a façade with nothing of its own), carries **no
standing claim**, and is `Root`-, `Anonymous`- or `Module`-kinded — the last
only when it announces nothing, its group record being the one part of that kind
the copy re-births. What stays unmodelled is a `Sig` scope's live slot collector
and a `Module` body's announced declaration window, so an environment carrying
either declines by the same gate as every other unmodelled environment rather
than by a special case. A **module value** declines for a different reason
entirely: only a `KFunction` reaches the consolidate verb, so a module's own
child scope is never offered to the gate
([module-scope-consolidation.md](../roadmap/foundation/module-scope-consolidation.md)).

Because the copy can fire at an arbitrary escape crossing, it may meet an
environment still under construction: an unfinalized binding, or a captured
scope whose defining block has not closed. The copy then pins rather than
parks — `Pin` is always sound, since retaining more never dangles. No wait edge
is ever added to the finalize walk, so the deadlock two mutually-referencing
in-flight environments would otherwise create is unconstructible. The engine
re-checks readiness itself before it allocates anything: the chooser's earlier
verdict is pricing, not authority.

## Inferred capture

`CLOSE (block)` infers the capture list: a structural walk over the block's raw
AST ([close_inference.rs](../src/machine/model/close_inference.rs)), run at the
form's step on the step scratch, yields the block's free identifiers on both
channels — value names and type names — callables and modules being implicit
already. The two `CLOSE` overloads share one resolve-park-seed spine
([close_over.rs](../src/builtins/close_over.rs)) and differ only in where the
list comes from. `CLOSE OVER ()` is an empty capture, a distinct bucket key from
inference.

### Which inferred names are captured

A free identifier that resolves in the per-call portion of the enclosing chain
is captured exactly as if written in a `CLOSE OVER` list (an in-flight claim
parks the build); one that resolves only in the eternal tier is read through the
block's outer link rather than copied, because a copy of it buys nothing. The
tier is read off the resolution that found the binding — `HitTier`
([resolve.rs](../src/machine/core/scope/resolve.rs)), split at the same
per-call/eternal predicate implicit close walks with — and the untiered
resolvers are *defined as* the tiered ones with the tier dropped, so `CLOSE` and
`CLOSE OVER (<the same names>)` can never pick different bindings for a name.

A name on **either channel** that resolves nowhere is an unbound-name error at
the form, matching explicit `CLOSE OVER`. The type channel holds that rule
because no type position spells a bare token that is not a chain-resolvable
name: a union's variant tag is reached only by member projection (`Maybe.Some`),
whose lhs is the resolvable name, and the arm heads of a `MATCH … OVER U`
resolve against `U`'s member schema with `U` itself the captured name
([user-types.md § Unions dissolve into per-variant
newtypes](typing/user-types.md#unions-dissolve-into-per-variant-newtypes)).

### Freeness is position-aware

An identifier is bound by a local binder only where the interpreter's own strict
`idx < cutoff` rule would bind it: a use lexically before a `LET` of the same
name is free, and a binder never binds its own right-hand side. A nested
statement block, a function body, an operator body and a match arm each restart
the count in a scope of their own, seeded with the names the surface *binds*
rather than spells — the signature's parameters, and the machine-fixed `left` /
`right` / `operands` / `it` (`MACHINE_BINDERS`,
[binder.rs](../src/machine/model/binder.rs)). A name in a label position — an
attribute's field in `m.x`, a projection's field list, a record literal's keys,
the name half of a `<name> :<Type>` pair — is not a use at all.

The type channel has two order-independent windows the position rule does not
describe, and the walk mirrors both:

- **Self-recursion.** A nominal declaration binds its declared **binder** inside
  its own representation — `NEWTYPE Tree = :{left :Tree}`. A `UNION`'s variant
  tags are not among them: a tag is a member label, reached only through its
  binder or by projection off it ([user-types.md § Unions dissolve into
  per-variant newtypes](typing/user-types.md#unions-dissolve-into-per-variant-newtypes)),
  so the walk reads a union schema at its `<Tag> <payload>` stride — labels on
  the tag half, uses on the payload half — and a payload spelled like a sibling
  tag names an outer type.
- **Module-body announcement.** A `MODULE` / `GROUP` body pre-announces its
  top-level nominal declarations body-wide
  ([`announce_type_members`](../src/machine/model/binder.rs)), so a mutually
  recursive pair declared inside the block is free in neither source order.

### Dynamic names, and the domain's frontiers

Two forms surface names dynamically, so their presence in the inference domain
is a structured error — the free identifiers cannot be identified and
`CLOSE OVER` remains the form that admits them:

- `$(...)`, and its spelled-out `EVAL` head: EVAL resolves names against the
  scope at the evaluation site.
- `USING … SCOPE (…)`: the window surfaces its module's members at run time, so
  a syntactic walk cannot tell a member reference from a name the enclosing
  chain must supply.

The inference domain is the walked region of the block, and its frontiers are:

- A `#(...)` quote in an eager position is data — nothing inside it resolves. A
  quote *filling a builtin's lazy code slot* is that builtin's body spelled with
  a quote, so it is walked as the body: `CLOSE #(x + 1)` infers `x`.
- A nested `CLOSE OVER` contributes the identifiers in its **capture list**,
  which resolve against this chain at the inner form's build, and nothing from
  its body — that block is severed.
- A nested `CLOSE` contributes the free set of its own block, but not its
  conflicts: it polices its own block when it evaluates.

Outside those excluded regions the walk is exact: only the fixed builtin forms
have lazy slots, so every remaining group in the domain evaluates in the block's
chain and structural freeness coincides with resolution. Those forms are
recognized by full untyped bucket key, sound for the same reason
[`BINDER_SPECS`](../src/machine/model/binder.rs) and
[`LAZY_SLOT_SPECS`](../src/machine/model/lazy_slots.rs) are: builtin buckets are
unshadowable, so a matching node can only ever resolve to that builtin's
overloads.

## Open work

- [Callable copy tuning](../roadmap/foundation/callable-copy-tuning.md) — the
  pricing levers the first seam leaves unpulled: foreign-crossing pricing (so a
  pin can be re-consolidated at a later crossing) and a measured justification
  for α.
- [Module scope consolidation](../roadmap/foundation/module-scope-consolidation.md)
  — an escaping module value pins its body region whether or not anything
  captured the body, and a `USING … SCOPE` window that aliases such a scope
  reads as not-ready; one surface, since the window is the alias.
- [Callable cells in copied containers](../roadmap/foundation/callable-cells-in-copied-containers.md)
  — a record of closures crossing the seam severs nothing for its cells, and the
  container's memoized copy cost counts no environment, so the consolidation is
  never even offered.
- [A flattened dispatch registration pins its defining frame](../roadmap/foundation/flattened-registration-pins-its-frame.md)
  — a registration implicit close flattens into a block scope captures the
  per-call frame that declared it, which is still open at the crossing, so its
  nested rebuild declines and the block holds that frame. An `OP` inherits it,
  being a dispatch registration as well as a registry entry.
