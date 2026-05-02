# Roadmap

Larger structural items that don't fit in a single PR. Each section names the problem, why it matters, and possible directions — not a fixed design.

The order matters. Sequencing here is purely about technical and design dependencies — Koan has no users yet, and won't until this roadmap is fully implemented, so backward-compatibility costs play no role in ordering. The cost being optimized is engineering rework: doing one item before another it depends on means doing the dependent item twice.

User-defined functions and the first-cut tail-call optimization have shipped, along with a structural detour — the dispatch-as-node scheduler refactor — that wasn't in the original plan but turned out to be the natural shape once user-fns surfaced what was wrong with the previous schedule-time-then-execute split. Those landings settled `BuiltinFn`'s shape (now `fn(&mut Scope, &mut dyn SchedulerHandle, ArgumentBundle) -> BodyResult { Value | Tail }`) and named the next concrete pressures: parameterized user-fns leak per-call (so the leak fix is now load-bearing for any real workload), and TCO's slot reuse only covers the outermost user-fn frame (so transient-node reclamation, which lives inside the leak fix, gates true O(1) recursive memory). The leak fix and error handling come next in that order — each revisits `BuiltinFn`'s return type, and folding the next design problem into the same pass keeps the rewrites cheap. Monadic side-effect capture lands as the third (and intended-final) revision to that signature, replacing the ad-hoc `Scope::out` channel with a uniform carrier. User-defined types and traits come last: with the dispatch priority comparator built and the calling convention settled, the type machinery gets designed against a stable substrate.

## A builtin for user-defined functions ✓

**Status: shipped.** Surface syntax `FN (<signature>) = (<body>)` where the signature is a `KExpression` mixing fixed `Keyword` tokens and `Identifier` slots that bind as `Any`-typed `Argument`s; the body is a `KExpression` evaluated at call time. `KFunction.body` is the enum [`Body { Builtin(BuiltinFn) | UserDefined(KExpression) }`](src/dispatch/kfunction.rs) — shape (a) from the original directions, chosen because the upcoming TCO and error-frame work both want to introspect user-fn bodies (a `Box<dyn Fn>` would have hidden them).

Calling convention is parameter substitution: `KFunction::invoke` rewrites parameter `Identifier`s in a clone of the body to `Future(call-site value)` and returns it as `BodyResult::Tail` for the scheduler to dispatch in place. Free names (anything not a parameter) resolve via the call-time scope chain — for top-level `FN` definitions this coincides with lexical scoping.

**Deferred.** True lexical closures (a user-fn returning another user-fn that closes over local state) need real per-call child scopes, which depend on the leak fix landing first; substitution doesn't carry the captured scope across function boundaries. Type annotations on parameter slots are a future extension that hangs off the existing signature parser. Variadic arguments — the original "function body is a sequence of expressions" sketch — still want a design pass; the comparator's tiebreak rule for variadic-vs-fixed signatures is the load-bearing question and is unchanged from the original roadmap.

## The dispatch-as-node scheduler refactor ✓

**Status: shipped, was not in the original roadmap.** The original architecture split dispatch across schedule time (eager dispatch in `schedule_expr`), execute time (`Pending` nodes), and inline-in-builtin-bodies (`if_then` and the original `KFunction::invoke` reaching for `scope.dispatch` directly). Three workarounds for one missing abstraction: only the schedule-time path could compose with sub-expression evaluation, so user-fn bodies with nested expressions silently nulled and forward references to user-fns required a try-eager-then-fallback hack.

The refactor made `Dispatch(KExpression)` a scheduler node type. `schedule_expr` collapsed to "add a `Dispatch` per top-level expression"; the rest is dynamic — `Dispatch` walks its expression's parts at run time, spawns sub-`Dispatch`/`Bind`/`Aggregate` nodes, and a builtin body that holds `&mut dyn SchedulerHandle` can also add `Dispatch` nodes (used by `if_then`'s lazy slot before TCO landed; now superseded by `Tail`). `BuiltinFn`'s return type became `BodyResult { Value(&KObject) | Tail(KExpression) }`. The `Forward(NodeId)` mechanism in the result vec lets a body whose result depends on a deferred computation defer cleanly.

This was the foundational change that made the rest of the user-fn and TCO work tractable. Worth recording in the roadmap because the next several items design against its shape (the leak fix has to cover scheduler-side allocations, the error story has to thread errors through the node graph, monadic effects need access to the same node-spawn lever).

## Tail-call optimization ✓

**Status: first cut shipped.** [`BodyResult::Tail(KExpression)`](src/dispatch/kfunction.rs) makes a builtin's tail return rewrite the *current scheduler slot's work* to a fresh `Dispatch(expr)` and re-run in place — no new node allocated, no `Forward` chain. Both deferring builtins (`if_then`, `KFunction::invoke` for user-fns) were tail by construction and migrated. A chain of tail calls (`A → B → PRINT` or unbounded `LOOP → LOOP`) reuses one slot end-to-end. Verified with two slot-count assertions in the test suite.

The roadmap's original concern — host-stack overflow on naïve recursion — was actually solved earlier by the dispatch-as-node refactor (every "recursive call" enters the FIFO queue rather than growing the Rust call stack). What `Tail` adds is constant *scheduler-vec* memory across the tail-call sequence.

**Deferred.** The `Tail` rewrite covers only the outermost slot. Body-internal sub-expressions — the predicate of an `IF`-guarded base case, the argument expressions to a recursive call — still allocate sub-`Dispatch` + `Bind` nodes per iteration, and those nodes are never reclaimed. Realistic recursive patterns (factorial, list walk) run in O(n) scheduler memory until the leak fix lands transient-node reclamation; the chain-of-tail-calls slot reuse alone isn't enough for them. The leak fix is the gating dependency.

## Replace `Box::leak` with arena-allocated `KObject`s — leak fix shipped, follow-ups pending

**Status: leak fix shipped via lexical closures + per-call arenas + Rc-counted closure escape.** `Box::leak` is gone from production code; per-call user-fn allocations (substituted body, child scope, parameter clones, in-body `LET`/`value_pass` allocations) live in a per-call `RuntimeArena` owned by `CallArena` in [`arena.rs`](src/dispatch/arena.rs) and freed when the call's slot finalizes — *unless* a closure that captured the per-call scope has escaped, in which case the `Rc<CallArena>` carried by the lifted `KObject::KFunction` keeps the arena alive for as long as the closure is reachable. Free names in user-fn bodies resolve through the FN's *captured* definition scope ([`KFunction.captured`](src/dispatch/kfunction.rs)) — lexical scoping for free vars, which broke the F_{k+1}→F_k chain that would otherwise have made TCO recursion O(n) memory. Top-level FNs capture the run-root, so behavior matches the old dynamic-scoping model for currently-expressible programs. First-class function values (returning a fresh FN from a body, binding it via `LET`, calling via `call_by_name`) work end-to-end as of Stage 3 of the closure-escape work.

The leak-fix regression test [`repeated_user_fn_calls_do_not_grow_run_root_per_call`](src/dispatch/builtins/fn_def.rs) confirms 50 ECHO calls grow run-root by exactly 50 (one lifted return value per call), down from the prior 250+.

**Follow-ups, ordered by severity.** These came out of an audit after the leak fix landed. The first is the only one that can become a real UAF (and only when Koan grows past current expressivity); the rest are correctness-fine but worth tracking.

1. ✓ **`deep_clone` is shallow for `KFunction(&KFunction)` references — closures now escape correctly.** Fixed via Option D staged in three parts:
   - **Stage 1**: `Box<CallArena>` → `Rc<CallArena>`. The slot's frame Rc drops on finalize; the underlying arena drops only when no Rc is held. Pure foundation refactor; same observable behavior.
   - **Stage 2**: `KObject::KFunction(&fn, Option<Rc<CallArena>>)` — variant gains a frame field that keeps the function's underlying per-call arena alive when the closure escaped. `None` for builtins and top-level FNs (run-root never dies); `Some(rc)` for closures that escaped a per-call body.
   - **Stage 3**: Lift logic at `Done(Value, Some(frame))` and in `finalize_ready_frames` checks whether the lifted KFunction's `captured_scope().arena` matches the dying frame's arena pointer. Match → carry an Rc clone in the lifted value. Mismatch → no Rc needed.
   - Tests at [src/dispatch/builtins/call_by_name.rs](src/dispatch/builtins/call_by_name.rs) (`closure_escapes_outer_call_and_remains_invocable`, `escaped_closure_with_param_returns_body_value`) confirm the previously-UAF closure-escape pattern now works.
   - **Still shallow for `KExpression(Future(&KObject))`** — same hazard, different variant. KExpression carries `Future(&KObject)` parts that may reference into a dying arena. Today no body returns a `KExpression` value (lazy IF slots get consumed before return), so it's not active. Stage 4 generalizes the Rc-carrying treatment to KExpression and the other reference-bearing variants when needed.

2. ✓ **Lift is unconditional — KFunction case resolved by Stage 3.** The new `lift_kobject` helper checks the function's arena and only attaches an Rc when needed; otherwise the lifted value's frame field stays `None` and the original reference is shared without per-call ceremony. For non-KFunction values (Number, KString, Bool, Null, List of those), `deep_clone` is still called unconditionally — but those are owned data, so the clone is correct (just slightly wasteful for the "value already in run-root" case). Full elimination of the unconditional `deep_clone` for those variants requires arena-provenance infrastructure that doesn't exist yet — addressed by Stage 4 if profiling justifies it.

3. ✓ **`finalize_ready_frames` was O(n²) over scheduler size.** Fixed via a sidecar `frame_holding_slots: Vec<usize>` in `Scheduler` updated on stash/finalize.

4. ✓ **`Scope::arena: Option<&'a RuntimeArena>` was `None` only for `test_sink()`.** Fixed by tightening the field to `&'a RuntimeArena` and giving `test_sink()` a caller-supplied arena.

5. ✓ **`active_scope: Option<*const Scope<'a>>` raw pointer dance.** Fixed by passing the running scope through `SchedulerHandle::add_dispatch(expr, scope)` directly; the field and the unsafe transmute are gone.

6. **Miri hasn't run.** `CallArena::new`'s heap-pin + lifetime-erasure transmutes match the existing `RuntimeArena::alloc_*` pattern, but they haven't been validated under Miri. Worth running once before declaring the unsafe blocks settled.

7. **`Scope::data`/`Scope::functions` use `RefCell<HashMap>`.** Re-entrant mutation during dispatch would panic. Current code paths don't do this, but a future builtin that iterates `scope.data` while a callback into `scope.add` is live would crash. Not a memory bug, but a footgun.

8. **The lift's "captured scope must have an arena" assumption.** `KFunction::invoke`'s `unwrap_or(scope)` fallback for when `captured_scope()` returns `None` is unreachable today (FN always captures), but accepted at runtime. A `debug_assert!(self.captured_scope().is_some())` in the user-defined branch would surface any future regression.

**Sibling concern: scheduler-vec growth.** The [`Scheduler`](src/execute/scheduler.rs)'s `nodes`/`results` vecs still grow per iteration whenever a body-internal sub-expression spawns a sub-`Dispatch`/`Bind`. Tail-call slot reuse covers only the outermost user-fn frame, so realistic recursion (the predicate computation in an `IF`-guarded base case, or a recursive call's argument expressions) accumulates entries. The `frame_holding_slots` mechanism added for #3 is one piece of the substrate; full transient-node reclamation is the larger remaining problem and gates true O(1) tail-recursive memory.

## Surface dispatch and type errors instead of swallowing as `Null`

**Problem.** Every error path currently produces `KObject::Null` (via [`null()`](src/dispatch/builtins.rs) returning `BodyResult::Value(null_kobject())`): `try_args!` mismatches in [builtins.rs](src/dispatch/builtins.rs), `Rc::try_unwrap` and shape mismatches inside [`if_then::body`](src/dispatch/builtins/if_then.rs) and [`fn_def::body`](src/dispatch/builtins/fn_def.rs), unbound names in [`value_lookup`](src/dispatch/builtins/value_lookup.rs), parameter-substitution mismatches inside user-fn bodies (a `Future` ending up where an `Identifier` was expected because a user shadowed a parameter with `LET`). Dispatch failures inside [`Scheduler::run_bind`](src/execute/scheduler.rs) and the recursive `scope.dispatch` call inside [`KFunction::invoke`](src/execute/scheduler.rs) propagate as `Result<NodeStep, String>` and surface at the top level as a stringly-typed error — better than `Null`, but unstructured.

User-defined functions sharpen the impact: a typo in a user-fn body produces a dispatch error several frames deep, and the current `String` error doesn't say *which frame*. The fix needs frame context, not just a flat enum.

**Impact.** Debugging requires reading the source of the builtin that just returned `Null`. As the language grows, this becomes the dominant friction during implementation work — every smoke test against a new builtin starts with "is this `Null` real or a swallowed bug?" Failures are silent and identical, so triaging which builtin in a chain produced the unwanted `Null` requires bisecting by hand.

**Directions.**

- Introduce a `KError` type (probably an enum: `TypeMismatch { arg, expected, got }`, `MissingArg(name)`, `UnboundName(name)`, `DispatchFailed(reason)`, plus a `User(String)` for in-language `raise`-style errors). Carry a `Vec<Frame>` for stack-of-frames context — user-fn bodies need to report which call frame, which expression.
- Change `BodyResult` from `Value(&KObject) | Tail(KExpression)` to `Value(&KObject) | Tail(KExpression) | Err(KError)`, or wrap as `Result<BodyResult, KError>`. Every builtin's `return null()` paths that mean "type mismatch" or "shape error" become `return Err(KError::...)`; intentional-null returns stay as `Value(null_kobject())`.
- The interpreter ([execute/interpret.rs](src/execute/interpret.rs)) decides what to do with `Err`: print and abort by default, or — once the language has try/catch-style constructs — bubble it up through the expression tree until a handler catches it. The scheduler's existing `String` error path through `run_bind`/`run_dispatch` becomes the carrier.
- The `try_args!` macro keeps its shape; the `return $err` clause is what each call site customizes anyway.

This pairs naturally with the leak fix — if we're already revisiting `BodyResult` for ownership, doing both at once avoids two churning passes.

## Generalize `Scope::out` into monadic side-effect capture

**Problem.** [`Scope::out`](src/dispatch/scope.rs) is a `Box<dyn Write>` sink that exists solely so [`PRINT`](src/dispatch/builtins/print.rs) has somewhere to send bytes and tests can swap stdout for a buffer. It is the only side-effect channel the runtime has, and it is hard-coded to one channel and one shape (write bytes). Every additional effect Koan eventually wants to support — file IO, time, randomness, network, environment access, even error reporting — would either grow `Scope` by another ad-hoc `Box<dyn ...>` field or get baked into `std::io` calls inside individual builtins.

Meanwhile the [`Monadic`](src/dispatch/ktraits.rs) trait already exists, with `pure` + `bind` over a `Wrap<T>` GAT, and its doc comment says it is "intended as the abstraction Koan's deferred-task and error-handling combinators will share once they're fleshed out." Today it is implemented only for `Option` and threaded through nothing in the runtime. It is scaffolding without a building.

**Impact.**

- *No effect inspection.* Tests can capture `PRINT` output by swapping the writer, but there is no equivalent for any other effect a builtin might want to perform. Each new effect requires its own bespoke testing seam.
- *No mocking or replay.* A program's behavior is whatever the host system decides at the moment of the call. Deterministic replay of a Koan program (feed it a recorded effect trace, get the same output) is impossible without a uniform effect channel.
- *No pure/effectful boundary.* The language has no way to know whether an expression is referentially transparent. Optimizations the scheduler could make (memoization, reordering, parallelism) are unsafe by default because any builtin might secretly write to a file or read the clock.
- *Effect ordering is implicit.* Today, effects happen in whatever order the scheduler runs builtins. There is no declarative "this expression's effect is X, sequenced after Y" — it is all operational.

**Directions.** None of these are decided.

- *Effect type.* Probably an enum: `Effect::Output(Vec<u8>)`, `Effect::Read(handle)`, `Effect::Now`, `Effect::Random`, plus a catch-all for builtins to declare custom effects. Open question: enumerated (closed set, easy to handle exhaustively) vs trait-object (`Box<dyn Effect>`, extensible by user code if/when user-defined functions can declare their own effects).
- *Carrier shape.* `BuiltinFn` returns not a bare `&'a KObject<'a>` (or `Result<...>` after the error-handling item lands) but an `Effectful<T>` carrier — a value paired with a list of pending effects. `Effectful` implements `Monadic`: `pure(v)` is `(v, [])`, `bind` concatenates effect lists. This is the long-promised second `Monadic` impl the trait's doc comment is waiting for.
- *Handler in `Scope`.* `Scope::out` becomes `Scope::handler: Box<dyn EffectHandler>`. The handler decides what to do with each `Effect` as the interpreter drains them: a default handler actually performs them (write to stdout, read the clock); a test handler captures them into a vec; a replay handler feeds results from a pre-recorded trace.
- *Drainage points.* Effects can either be performed eagerly (handler runs them as each builtin returns) or lazily (collected up the tree and run in batches at top-level expression boundaries). Eager is simpler and matches today's behavior; lazy unlocks reordering and is closer to the "monad transformer stack" shape this is converging on. Pick one explicitly rather than letting it emerge.

**Sequencing.** `BodyResult` already absorbed one revision (`Value | Tail` for TCO); the error item adds a second (`Result<BodyResult, KError>` or an `Err` variant) and this one adds a third (`Effectful<...>`). Three churning passes over every builtin in [builtins/](src/dispatch/builtins/) is meaningfully worse than one. Unless the effect story sharpens enough to fold into the same pass as ownership and errors, this should land last and accept that the prior two items are stepping stones rather than end states.

## User-defined types and traits

**Problem.** [`KType`](src/dispatch/kfunction.rs) is a closed enum of seven host-defined variants — `Number`, `Str`, `Bool`, `Null`, `Identifier`, `KExpression`, `Any`. Its doc comment already flags the limitation: *"In the future this should not assume all types can be enumerated; the user should be able to define duck types."* [`KObject::UserDefined`](src/dispatch/kobject.rs) exists as a unit-variant placeholder pointing at the same eventual feature, with no fields and no machinery behind it. Koan code today cannot define a record, a tagged union, a trait, or any abstraction over its own values.

**Impact.**

- *User functions can only operate on built-in types.* Now that user-defined functions exist, the language can express a function over `Number` but not over a `Point` — `Point` has no surface syntax because user types don't exist. The function feature is operationally usable but stuck at scalars and the built-in `List`/`Dict`. There is no path from "the language has a function abstraction" to "the language has a record abstraction the function can operate on."
- *No abstraction over types.* Writing a function over "anything that can be iterated" or "anything that can be compared" requires a trait or contract — Koan has no way to express either. The host-side [`ktraits.rs`](src/dispatch/ktraits.rs) (`Parseable`, `Iterable`, `Monadic`, etc.) gives the runtime its own vocabulary; user code is denied the analog and has to write per-concrete-type variants of every function.
- *Dispatch priority is built on the wrong model if types land later.* With seven host types, signature specificity is a tiny finite-set comparison. With user types, specificity becomes a partial order over a lattice that grows as user code grows — subtyping, trait satisfaction, and structural matching each want different specificity rules. A priority comparator designed for the closed-enum case is not the same comparator needed for the open-lattice case.

**Directions.** None decided.

- *Type representation.* Move `KType` from a closed enum to an extensible structure. Either add a `KType::User(TypeId)` variant alongside the existing host types and keep a `Scope`-level registry of definitions, or replace the enum entirely with a trait-object that host types and user types both implement uniformly. The first is incremental; the second is cleaner but a bigger refactor.
- *Surface syntax.* Type definitions and trait definitions are themselves builtins — likely `TYPE Point = STRUCT x:Number y:Number` and `TRAIT Iterable = ...` shapes. Mechanically these are `KFunction`s with fixed signatures, so the surface design echoes (and likely shares machinery with) the FN signature work in the user-functions item.
- *Traits.* A trait is a named bag of operation signatures that a type can claim to implement. Functions accept a trait-typed parameter and dispatch over any concrete type satisfying it. The dispatch machinery sees a trait the same way it sees a parent type in a subtyping hierarchy — a less-specific match that concrete types beat. The priority rules need a "concrete > trait > `Any`" hierarchy reserved in their design even if traits don't ship in the first cut.
- *Wiring up `KObject::UserDefined`.* The placeholder variant becomes something like `UserDefined(TypeId, HashMap<String, KObject>)` — a tagged record carrying a type identifier and field values. Other `KObject` variants stay as-is; user types are an addition, not a replacement.
