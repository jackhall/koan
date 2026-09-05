# Monadic side effects

**Problem.** The runtime has exactly one effect channel:
[`RunWriter`](../../src/machine/core/arena/frame.rs), a
`RefCell<Box<dyn std::io::Write>>` on the run frame, reached as
`ctx.out.write_out(bytes)`. It exists so [`PRINT`](../../src/builtins/print.rs) has
somewhere to send bytes and tests can swap stdout for a buffer. It is hard-coded to one
channel and one shape — write bytes — and its write errors are dropped, because a builtin
has nowhere to put them. Every further effect Koan wants — file IO, time, randomness,
network, environment access — would either add another `Box<dyn …>` field to the frame or
bake `std::io` calls into individual builtins. Nothing in the runtime carries a value
*paired with* a pending effect: a builtin performs its effect inline and returns a plain
carrier.

The type system cannot yet express the interface that would replace it.
[design/effects.md](../../design/effects.md) specifies a `Monad` signature, but its `pure`
and `bind` are written at `Number` because a signature member cannot quantify over the
element type. A bodyless `FN` head refuses a return type naming one of its own parameters
([modules.md § Keyworded members](../../design/typing/modules.md#keyworded-members)), and a
`VAL` slot takes a closed `KFunction` whose parameter and return types are already resolved.
The higher-kinded half has shipped — `TYPE (Type AS Wrap)` declares a constructor slot,
`:(Number AS Wrap)` applies it, and opaque ascription mints a per-call `TypeConstructor`
member so two ascribing modules carry distinct `Wrap` identities
([functors.md § Higher-kinded type slots](../../design/typing/functors.md#higher-kinded-type-slots)).
What is missing is the quantification that makes `Monad` one signature rather than a family
of per-element ones.

**Acceptance criteria.**

- A single `Monad` signature in Koan types `pure` and `bind` polymorphically in the element
  type: one ascription admits a module at every element type, and `bind`'s element change
  (`:(Elt AS Wrap)` to `:(Res AS Wrap)`) is expressible within that one signature.
- `Random`, `IO`, and `Time` each ascribe `Monad` and add their own operations, and every
  builtin that emits a side effect calls one of them rather than touching a frame field.
- A test that swaps the ascribed module observes mocked output, and a run replayed from a
  recorded trace produces the identical value sequence.
- A failed `PRINT` write reaches Koan code through the `IO` module's result rather than
  being dropped.
- A function whose parameter list names no `Monad`-ascribing module is classified
  referentially transparent from its signature alone, and the scheduler reads that
  classification.
- Module-system stage 4's generators draw randomness through the `Random` module.
- `RunWriter` no longer exists on the run frame, and no builtin holds a writer.

**Directions.**

- *Quantifying a member over the element type — decided per
  [Expression shapes are their own kind of function](../type_language/expression-shapes.md).*
  `pure` and `bind` are **quantified members**: the shape binds its own `:Type` parameters and
  declares the operation at every choice of them, so `Monad` is one signature and one ascription
  admits a module at every element type. That item owns the representation (a quantifier list on
  the shape type) and the satisfaction-time solve; this one writes the signature against it.
- *Standard effect modules — decided.* `Random`, `IO`, `Time`, with the existing
  effect-emitting builtins folded into `IO`. Each ascribes `Monad` and adds per-effect
  operations on top of the inherited `pure` and `bind`.
- *Runtime carrier — open.* Either widen the existing builtin return channel so a returned
  carrier can name pending effects, or add a parallel effect field the run loop drains.
  Recommended: widen the existing channel — the return path already carries every other
  builtin outcome shape, and a second field duplicates its lifetime and drain ordering.
- *Handler on the frame — decided.* `RunWriter` becomes a handler the run frame holds:
  default performs each pending effect, test captures them, replay feeds them from a
  recorded trace. Effect-performing code names the effect; the handler decides what
  happens.
- *Drainage points — open.* Eager (the handler runs effects as each builtin returns) or
  lazy (effects collect up the tree and run at scheduler boundaries). Eager is simpler;
  lazy leaves room for reordering. Pick one explicitly.
- *Purity classification — open.* Either derive the verdict on demand by reading the
  function's parameter record for a `Monad`-satisfying type, or store a bit on the
  function's schema at binder construction. Recommended: derive it, matching the
  return-slot classification functors already use
  ([functors.md § Generativity](../../design/typing/functors.md#generativity)).

## Dependencies

This is a currency revision sweeping every builtin in [builtins/](../../src/builtins), so it
folds naturally into the eventual static-typing/JIT pass if their schedules align.

**Requires:**

- [Expression shapes are their own kind of function](../type_language/expression-shapes.md) — the
  `Monad` signature's `pure` and `bind` are quantified members, which that item's shape type
  carries. The other half of the surface — the `TYPE (Type AS Wrap)` constructor slot — has
  shipped.

**Unblocks:**

- [Standard library](../libraries/standard-library.md) — the standard effect modules ship as
  stdlib entries.
- [Module system stage 4 — Property testing and axioms](../predicate_typing/axioms-and-generators.md)
  — generators thread randomness via the `Random` effect module.
