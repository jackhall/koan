# Module system stage 1 — Module language

**Problem.** [The module-system design doc](../design/module-system.md) describes
a module-based abstraction system that the language doesn't yet have. Stage 1
lays the foundation: surface syntax for **structures** (modules) and
**signatures** (module types), the **ascription** machinery (both transparent
and opaque), and per-module **type identity** so an opaquely-ascribed
`IntOrd.t` is genuinely distinct from `Number` even when its underlying
definition is `Number`.

This is the largest single stage by engineering effort. Until it ships, the
design doc describes a system whose only realization is the doc itself.

**Impact.**

- *Abstraction barriers.* Modules hide their representation behind a
  signature — consumers see only the operations the signature exposes, and
  an opaquely-ascribed `IntOrd.t` is genuinely distinct from `Number` even
  when its underlying definition is `Number`.
- *Namespacing.* `Set.add` and `List.add` coexist as bare names; methods no
  longer have to be globally-unique free functions in one shared top-level
  scope.
- *Per-module type identity has a carrier.* A `KType::ModuleType { module_path,
  name }` (or analog) variant lands alongside the existing host types in
  [`KType`](../src/dispatch/types/ktype.rs), plus a per-scope module registry
  so the type system can talk about "the `t` inside module `M`." This is also
  the dispatch hook that lets methods attach per-type rather than as
  globally-unique free functions.

**Directions.** None decided.

- *Surface syntax.* Koan's keyword-heavy convention (FN, LET, MATCH) suggests
  `MODULE`, `SIG`/`STRUCT`, and a pair of distinct ascription operators for
  transparent vs opaque. A concrete proposal lands at the start of this
  stage. The design doc uses OCaml-style placeholders.
- *File-to-module mapping.* The simplest rule is "one source file is one
  module, named after its filename"; nested modules inside the file use the
  in-language syntax. The remaining file-layout questions are owned by
  [files and imports](files-and-imports.md), which depends on this stage.
- *Type identity carrier.* Add a `KType::ModuleType { module_path, name }`
  (or similar) variant alongside the existing host types, plus a per-scope
  module registry.
- *Inference-as-scheduler-node.* The compiler's type-checking infrastructure
  starts here. Decide the scheduler's phase boundary (when type-checking ends
  and evaluation begins) and how multi-target unification is modeled
  (out-of-band substitution vs type-variable nodes that get refined and woken
  up). See [the design doc's compile-time scheduling
  section](../design/module-system.md#compile-time-scheduling).
- *What ascription enforces.* Transparent ascription is name- and
  shape-checking; opaque ascription is the same plus representation hiding.
  Type identity for opaquely-ascribed types is the load-bearing decision.

## Dependencies

**Requires:** none. The pre-module cleanup (vestigial `KType::TypeRef` removal,
ordered struct values, centralized constructor dispatch, and the `TypeResolver`
trait threaded through `KType::from_type_expr`) is shipped substrate; the
type-identity-carrier change here is now an additive change at the resolution
path and dispatch table rather than a broad rewrite.

**Unblocks:**
- [Stage 2 — Functors](module-system-2-functors.md)
- [Stage 4 — Property testing and axioms](module-system-4-axioms-and-generators.md)
- [Files and imports](files-and-imports.md)
- [Post-stage-1 Miri audit redo](post-stage-1-audit-redo.md) — the type-identity
  carrier and per-scope module registry reshape the memory model the audit signs
  off on, so the slate gets re-run after this stage ships.
