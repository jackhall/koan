# Files and imports

**Problem.** [`main.rs`](../src/main.rs) reads one source string — a file path
argument or stdin — and that is the entire Koan program. There is no way for
one Koan source file to reference definitions in another: no import, no
qualified path, no project-level entry point. A Koan codebase is one file.
Realistic programs outgrow that long before they outgrow a few hundred lines,
and the language cannot represent its own standard library as separate files
because the standard library does not yet exist as Koan code at all.

The [module system](../design/module-system.md) gives Koan an in-language
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

**Directions.** None decided.

- *File-to-module mapping.* The OCaml convention is "every file is a module
  of the same name" — `utils/list.koan` defines a module `List` inside a
  module `Utils`. The alternative is that files are containers for explicit
  `MODULE … END` declarations and aren't themselves modules. The first is
  cheaper to write and matches the most natural mental model; the second
  gives more control at the cost of boilerplate. A hybrid (each file is
  *implicitly* a module, and explicit `MODULE` declarations may nest inside
  it) is also viable.
- *Filesystem layout.* Flat directory of `.koan` files, or a tree
  (`utils/list.koan`, `utils/string.koan`)? Implicit entry point
  (`main.koan`) or explicit manifest file? Single-file programs (today's
  shape) should keep working — directory mode is an addition.
- *Import surface.* An explicit `IMPORT "utils/list"` builtin that loads the
  file and brings its top-level module into scope, vs. implicit "everything
  in the project directory is visible." Explicit is more verbose but makes
  the dependency graph readable; implicit is cheaper to write but couples
  every file to every other.
- *Qualified vs unqualified names after import.* `Utils.List.map` keeps
  collisions controlled and signals where a name comes from at the call
  site; an `OPEN Utils.List` form (or import-with-binding) lets the user
  drop the qualification when they want it. Two files each defining a
  `Point` type are the load-bearing case — module-system opaque ascription
  already gives them distinct identities, but the surface still needs a way
  to disambiguate at use site.
- *Definition vs side-effect at file load.* Does loading a file run its
  top-level expressions (so importing has effects), or only register its
  `FN`, module, and signature definitions and leave expression evaluation to
  the entry-point file? The latter matches most languages and dovetails with
  the monadic-effect work — effectful initialization wants the same handler
  machinery as effectful builtins.
- *Circular imports.* Disallow (simplest, may force awkward splits), allow
  with forward-declaration discipline, or resolve via the existing
  dispatch-as-node scheduler by treating cross-file references as another
  deferred dependency.

## Dependencies

**Requires:**

**Unblocks:**
- [Standard library](standard-library.md) — the stdlib lives across
  multiple `.koan` files; user code needs an import surface to load them.

Otherwise mostly orthogonal to the effect and error work — the file loader
uses whatever `BuiltinFn` signature exists at the time, and downstream
features (effects, the eventual checker) use whatever loader exists at
the time. Lands cleanly any time after the module language is in place.
