# Files and imports

**Problem.** [`main.rs`](../src/main.rs) reads one source string — a file path
argument or stdin — and that is the entire Koan program. There is no way for
one Koan source file to reference definitions in another: no import, no
qualified path, no project-level entry point. A Koan codebase is one file.
Realistic programs outgrow that long before they outgrow a few hundred lines,
and the language cannot represent its own standard library as separate files
because the standard library does not yet exist as Koan code at all.

The [module system](../design/typing/modules.md) gives Koan an in-language
notion of modules — structures, signatures, and ascription — but says nothing
about how source files map onto that notion or how one file reaches into
another. This item closes that gap.

**Impact.**

- *Decomposition.* Programs split across multiple files — related groups of
  functions and types live in their own files instead of cramming everything
  into one or pushing it down into Rust as a builtin.
- *Standard library in Koan itself.* "List utilities," "string helpers," and
  other naturally-Koan-expressible code ships as `.koan` files rather than
  Rust builtins, putting the right code at the right layer.
- *Per-file privacy boundary.* When each file becomes a module (or contains
  modules), the signature/ascription machinery from the module system gives
  exports a syntactic anchor; names stop having to globally not collide
  across the whole codebase.
- *Tests live alongside code.* A test file referencing the function it tests
  becomes expressible — the default shape of a test suite in every other
  language.

**Directions.**

- *File-to-module mapping — open.* The OCaml convention is "every file is a
  module of the same name" — `utils/list.koan` defines a module `List`
  inside a module `Utils`. The alternative is that files are containers for
  explicit `MODULE` declarations and aren't themselves modules. The first
  is cheaper to write and matches the most natural mental model; the second
  gives more control at the cost of boilerplate. A hybrid (each file is
  *implicitly* a module, and explicit `MODULE` declarations may nest inside
  it) is also viable.
- *Filesystem layout — decided.* A directory tree of `.koan` files
  (`utils/list.koan`, `utils/string.koan`, …); flat layout is rejected as
  it doesn't scale past a handful of files. Single-file programs (today's
  shape) keep working — tree mode is an addition. Implicit entry point
  (`main.koan`) vs. explicit manifest is a sub-question to settle when the
  first multi-file program lands.
- *Import surface — decided.* An explicit `IMPORT "utils/list"` builtin
  loads the file and brings its top-level module into scope. Implicit
  "everything in the project directory is visible" is rejected: the
  per-file dependency graph stays readable, and the file loader has a
  single concrete trigger to drive scheduler work off.
- *Qualified vs unqualified names after import — open.* `Utils.List.map`
  keeps collisions controlled and signals where a name comes from at the
  call site; an `OPEN Utils.List` form (or import-with-binding) lets the
  user drop the qualification when they want it. Two files each defining a
  `Point` type are the load-bearing case — module-system opaque ascription
  already gives them distinct identities, but the surface still needs a way
  to disambiguate at use site.
- *Definition vs side-effect at file load — open.* Does loading a file run
  its top-level expressions (so importing has effects), or only register
  its `FN`, module, and signature definitions and leave expression
  evaluation to the entry-point file? Recommended: the latter matches most
  languages and dovetails with the monadic-effect work — effectful
  initialization wants the same handler machinery as effectful builtins.
- *Circular imports — decided.* Resolved via the existing dispatch-as-node
  scheduler by treating cross-file references as another deferred
  dependency, consistent with the
  [dispatch-time name placeholder](../design/execution-model.md#dispatch-time-name-placeholders)
  mechanism. Disallowing or requiring forward-declaration discipline is
  rejected — the scheduler already handles deferred resolution generically
  and forcing source order on multi-file projects is gratuitous.

## Dependencies

**Requires:**

**Unblocks:**

- [Standard library](standard-library.md) — the stdlib lives across
  multiple `.koan` files; user code needs an import surface to load them.

Otherwise mostly orthogonal to the effect and error work — the file loader
uses whatever `BuiltinFn` signature exists at the time, and downstream
features (effects, the eventual checker) use whatever loader exists at
the time. Lands cleanly any time after the module language is in place.
