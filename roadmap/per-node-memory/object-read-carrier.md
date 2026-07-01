# Object and type read-site carrier

Both value and type reads carry each binding's reach forward from its bind site instead of
re-deriving a carrier from the value.

**Problem.** A value or type read back by name or ATTR member reconstructs its carrier from the
value rather than carrying reach forward from where the value was built.

*Object channel.* [`Scope::resident_object_carrier`](../../src/machine/core/scope.rs) wraps an
already-built `&'a KObject` as `Witnessed::new(.., FrameSet::singleton(home))` — six callers: FN def
([`fn_def/finalize.rs`](../../src/builtins/fn_def/finalize.rs)), name resolution
([`scope.rs`](../../src/machine/core/scope.rs)), ATTR member access
([`attr.rs`](../../src/builtins/attr.rs), two sites), the LET object RHS
([`let_binding.rs`](../../src/builtins/let_binding.rs)), and the literal Resolved arm
([`dispatch/literal.rs`](../../src/machine/execute/dispatch/literal.rs)). The re-wrap exists because
[`Bindings`](../../src/machine/core/bindings.rs) stores `data` as `(&'a KObject, BindingIndex)` — no
carrier, so a lookup has no reach to return and asserts a single-frame one instead. A deep clone
cannot recover an object's reach the way a module's is recovered (`KObject::deep_clone` shares its
inner `&KFunction` / `Rc` payloads, and a `KObject` exposes no bounded per-value reachability), so
the reach must travel with the binding.

*Type channel.* The type read clones into the read-site region and rebuilds the witness from the
value: [`Scope::seal_type`](../../src/machine/core/scope.rs) over `alloc_ktype_witnessed(kt.clone())`,
whose [`Scope::seal_module`](../../src/machine/core/scope.rs) arm walks a module's `child_scope()` on
every read to recover its reach. `Bindings::types` stores `(&'a KType, BindingIndex)` — again no
carrier — so the reach is reconstructed from the value each time rather than carried from the bind.

The FN-def / LET-RHS **define** sites likewise hand out a bare `&'a` and re-wrap it into a carrier
downstream, so the registered reference and the carrier come from two separate steps.

**Acceptance criteria.**

- `Bindings` carries each value binding's reach (its `FrameSet`) and each type binding's reach; a
  name or ATTR lookup — object or type — returns a witnessed carrier built from that stored reach.
  This realizes the bind-site reach-set of
  [§Storage and access](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into).
- Every read witnesses the existing region-resident `&'a KObject` / `&'a KType` in place from its
  stored reach. No site rebuilds a witness by walking a value:
  [`Scope::resident_object_carrier`](../../src/machine/core/scope.rs)'s asserted `Witnessed::new`,
  [`Scope::seal_module`](../../src/machine/core/scope.rs)'s `child_scope()` walk, and the type read's
  `alloc_ktype_witnessed(kt.clone())` re-clone no longer exist. This retires the transitional
  `resident_object_carrier` named in
  [§Construction](../../design/per-node-memory.md#construction-yoke-merge-map-and-one-wrapper-per-node).
- A value's or type's reach is computed once, at its define/finalize site, from the value's parts
  held directly there — an object's delivered RHS/arg carrier, a module's child scope held as a local
  `&Scope` — never by walking the built value, and stored on the binding. A freshly-built FN-def /
  LET-object-RHS object allocates and registers through its defining scope's frame-lifetime `&'a` (the
  co-located-resident plumbing) and seals its *terminal* carrier through the confined witnessed
  surface, so the registered reference and the returned carrier share one allocation and
  `Witnessed::resident` is never called from a builtin.

**Directions.**

- *Object read-site carrier — decided.* Store each value binding's reach on the classified
  `Bindings` entry — the value/type split that hangs it is structural (a value bind and a type bind
  of one name are mutually exclusive by construction), so a name / ATTR lookup returns a witnessed
  carrier instead of re-wrapping a bare `&KObject`.
- *Object define-site — decided.* Allocate + register the value through the defining scope's
  frame-lifetime `&'a` — the co-located-resident plumbing [`RegionBrand`](../../src/machine/core/arena.rs)
  reserves for binding entries, distinct from the per-alloc brand it reserves for terminals — and seal
  only its *terminal* carrier through the confined witnessed surface. Registration is a structural
  resident that outlives any brand window, so it stays `&'a` (the tight `content == borrow == 'a`
  resident, not a free lifetime) rather than moving into a brand; `Witnessed::resident` is never called
  from a builtin. Build+register-inside-the-brand was considered and rejected: it fights that split and
  drags the FN signature's invariant `&'a KType`s into a multi-operand brand re-anchor.
- *Carrier bundling primitive — decided.* Both the read and the define terminal carrier bundle their
  already-`&'a` value + reach through one confined witnessed/arena builder that does `resident` +
  `reseal_under` (witness = the value's `region_owner` home ∪ its binding's stored reach) — reusing the
  existing born-and-seal idiom (`alloc_object_witnessed` → `seal_value`), so `Witnessed::new` is not
  used and `Witnessed::resident` is never called from a builtin. A purpose-named `bundle_resident`
  constructor (redundant with `resident` + `reseal_under`) and a home-deferred foreign-reach-only
  carrier (not self-contained) were considered and rejected.
- *Type read-site carrier — decided.* Store each type binding's reach on the `Bindings::types` entry
  and witness the existing `&'a KType` in place, retiring the read-side `seal_module` `child_scope()`
  walk and the read clone — the type-channel mirror of the object read.
- *Type reach computed at define — decided.* Compute a type's foreign reach once, at its
  register/finalize site (the module-reach computation factored out of the read-side `seal_module`),
  and store it on the binding.
- *Module reach born on the carrier at construction; `seal_module` deleted — decided.* Every
  module-construction site — MODULE finalize
  ([`module_def.rs`](../../src/builtins/module_def.rs)) and the `:|` / `:!` views
  ([`ascribe.rs`](../../src/builtins/ascribe.rs)) — holds its child scope **directly** as a local
  `&Scope`, so `module.child_scope()` is never needed to recover it. The module carrier folds that
  child scope's reach (its `region_owner` ∪ sealed `reach`-set, home-omitted) at construction, and the
  home-omitted reach is stored on the `types` binding. Reads retrieve it via `resident_type_carrier`,
  and the idempotent-finalize guard reads the stored reach too — so nothing walks the `KType::Module`
  value to rebuild a witness, and [`Scope::seal_module`](../../src/machine/core/scope.rs) (with
  `seal_type`'s Module dispatch) is deleted.

## Dependencies

**Requires:**

- [The honest single-region witness substrate](../../src/witnessed.rs) — the define-site alloc + seal builds on the honest `yoke` witness and its `into_set` lift.

**Unblocks:**

- [Witnessed type and region operands](type-operand-carriers.md) — the capstone's `Witnessed::new` deletion needs this item's object-read callers retired.
