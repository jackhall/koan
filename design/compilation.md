# Compilation: residual code generation from the stalled DAG

Koan compiles by partial evaluation, not by a separate static compiler.
The build-time phase of two-phase execution
([execution/scheduler.md § Pegged and free execution](execution/scheduler.md#pegged-and-free-execution))
runs free execution with input nodes pegged until the scheduler stalls at
its fixed point; a code generator then consumes that stalled DAG state —
`NodeStore`, `DepGraph`, the pegged-node identifiers — as its
intermediate representation and emits ahead-of-time code for the
residual program. The interpreter is the compiler's front end: no
separate analysis pass reimplements elaboration, dispatch, or
name-placeholder semantics as static approximations. Whatever the
scheduler resolved is compiled; whatever it could not resolve falls back
to the same runtime machinery the interpreter uses today.

## Why the stalled DAG is the right intermediate representation

At the free-execution fixed point, the dynamism that makes surface koan
hard to compile has already been evaluated out by tested code:

- Every name is resolved through the lexical-visibility rules
  ([execution/name-placeholders.md](execution/name-placeholders.md));
  forward-reference parking and out-of-order sibling execution have been
  discharged, so the residual has no pending lookups.
- Type elaboration has run
  ([typing/elaboration.md](typing/elaboration.md)); there are no
  `Unresolved` transients left outside pegged regions.
- Operator and overload registries are fully populated for every scope
  the build phase executed, so dispatch shape classification and
  specificity ranking
  ([typing/ktype/](typing/ktype/README.md)) have concrete
  candidate sets to rank against.
- Every dispatch node whose inputs are build-time-known carries its
  `ResolveOutcome`; every fully-finalized node is a constant.

The code generator therefore compiles a much simpler language than
surface koan: no forward references, no pending types, dispatch mostly
resolved. The behaviors the DAG scheduler exists to provide at run time
— parking, replay on pending types, topological interleaving of
siblings — are consumed during the build phase, which is exactly what
licenses removing the runtime scheduler from compiled regions.
Tail-call slot rewriting
([tail-call-optimization.md](tail-call-optimization.md)) becomes native
tail calls or loops. The per-node interpretive overhead the scheduler
pays to buy those behaviors
([execution/calls-and-values.md](execution/calls-and-values.md)) is the
cost this design removes.

## The residual and its three call tiers

The residual — pegged nodes and everything data-dependent on them —
compiles to code in which every call site lands on one of three tiers:

1. **Direct call.** Dispatch resolved during the build phase; the
   generator emits a call to the compiled target, with argument slots
   bound per the picked overload (including its lazy `:KExpression`
   slots, which compile to thunks of the raw operand).
2. **Table dispatch.** Sites whose argument types are not
   build-time-known (untyped or `:Any`-flowing slots) compile to calls
   into the runtime library's dispatch machinery — the same
   candidate-bucket resolution the interpreter uses.
3. **Interpreted.** Regions downstream of a runtime-dependent `EVAL`
   barrier (below) execute in an embedded evaluator.

The runtime library ships in every compiled artifact regardless of tier
mix, because types are runtime values by design: `TYPE OF`, per-call
generative identities, opaque-ascription nonces
([typing/type-identity.md](typing/type-identity.md)), and
constructor application from runtime arguments all require the full
`KType` reification at run time. Compilation removes interpretation
overhead; it never erases the type system.

## The `EVAL` partition

`EVAL` ([metaprogramming.md](metaprogramming.md)) splits cleanly across
the build/run boundary:

- An `EVAL` whose operand's dependencies finalize during free execution
  splices at build time and compiles away entirely — the spliced
  declarations are ordinary nodes in the stalled DAG.
- An `EVAL` whose operand depends on pegged input pegs in turn. The
  block-level `EVAL` barrier already scopes what such a splice can
  affect: later siblings in the block park on it, so exactly that
  downstream region falls to the interpreted tier. The embedded
  evaluator exists for these regions and nothing else.

## Typing density buys call directness

Inside a function body, build-time dispatch resolution reaches exactly
as far as declared types do: a fully-typed signature gives the build
phase the carried types it needs to resolve body call sites per
signature, landing them on the direct tier. Untyped and `:Any` slots
stay on the table-dispatch tier. Annotation density therefore maps
directly onto the fraction of direct calls in the compiled output — a
gradual payoff requiring no new language surface.

## Effect order: one linearization is a legal refinement

Within a block, observable effect order is constrained only by data
dependencies and the `EVAL` barrier; siblings otherwise interleave at
scheduler discretion
([execution/calls-and-values.md](execution/calls-and-values.md)).
Compiled code fixes one topological linearization of each block. That
is a refinement of the permitted orders, not a semantic change: values
are immutable, and effects flow along the same dependency edges the
linearization respects.

## The serialization boundary

The load-bearing engineering is shared with the resumable snapshot: the
stalled state holds region-allocated, lifetime-erased references
(`&KType` values, scope-id-keyed nominal identities, `Rc<CallFrame>`
anchors — [memory-model.md](memory-model.md)). Emitting finalized
values as compiled constants and rehydrating type identities across the
build/run boundary is what makes the artifact self-contained.
Digest-keyed type identity
([typing/type-identity.md](typing/type-identity.md)) is what makes
rehydration well-defined: an identity minted during the build phase and
the same declaration elaborated at run time agree by content digest,
not by pointer.

## Two consumers, one fixed point

The stalled DAG state feeds two back ends, and neither turns it into a
bytecode IR:

- The **resumable snapshot** serves the development loop: build-time
  errors, editor data, and run-time resume by the same scheduler
  engine.
- The **code generator** serves deployment: the residual compiled
  against the runtime library, with the evaluator embedded only for
  runtime-`EVAL` regions.

The degradation envelope follows from the tier mix. Worst case —
untyped, runtime-`EVAL`-heavy code — compiled output is control flow
plus table dispatch everywhere: a transpiler with a runtime library.
Best case — typed code with a static top level — output approaches
sealed-world directness without sealing the language: open dispatch,
first-class types, and `EVAL` all keep their semantics, and the
compiler's reach is decided by how much of the program the build phase
could settle.

## Open work

- [Two-phase execution](../roadmap/editor_tooling/two-phase-execution.md)
  — the build-time phase, peg-set, and snapshot this design consumes;
  the residual code generator is tracked there as a direction.
