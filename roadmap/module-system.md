# Module system and directory layout

**Problem.** [`main.rs`](../src/main.rs) reads one source string — a file path argument
or stdin — and that is the entire Koan program. There is no way for one Koan source file
to reference definitions in another: no import, no module path, no project-level entry
point. A Koan codebase is one file. Realistic programs outgrow that long before they
outgrow a few hundred lines, and the language cannot represent its own standard library
as separate files because the standard library does not yet exist as Koan code at all.

**Impact.**

- *Decomposition.* Programs split across multiple files — related groups of functions
  and types live in their own modules — instead of cramming everything into one file or
  pushing it down into Rust as a builtin.
- *Standard library in Koan itself.* "List utilities," "string helpers," and other
  naturally-Koan-expressible modules ship as `.koan` files rather than Rust builtins,
  putting the right code at the right layer.
- *Private/exported boundary.* Per-file privacy gets a syntactic anchor; names stop
  having to globally not collide across the whole codebase.
- *Tests live alongside code.* A test file referencing the function it tests becomes
  expressible — the default shape of a test suite in every other language.

**Directions.** None decided.

- *Filesystem layout.* Flat directory of `.koan` files, or a tree (`utils/list.koan`,
  `utils/string.koan`)? Implicit entry point (`main.koan`) or explicit manifest file?
  Single-file programs (today's shape) should keep working — directory mode is an
  addition.
- *Import surface.* An explicit `IMPORT "utils/list"` builtin that loads and registers
  another file's definitions, vs. implicit "everything in the project directory is
  visible". Explicit is more verbose but makes the dependency graph readable; implicit
  is cheaper to write but couples every file to every other.
- *Namespacing.* Qualified names (`list::map`) keep collisions controlled and signal where
  a name comes from at the call site; flat naming with shadowing rules is simpler but
  re-creates the global-scope problem at codebase scale. Trait/type names are the
  load-bearing case — two modules each defining a `Point` type need a story.
- *Definition vs side-effect at module load.* Does loading a module run its top-level
  expressions (so importing has effects), or only register its `FN` and `TYPE`
  definitions and leave expression evaluation to the entry-point file? The latter matches
  most languages and dovetails with the monadic-effect work — effectful module
  initialization wants the same handler machinery as effectful builtins.
- *Circular imports.* Disallow (simplest, may force awkward splits), allow with
  forward-declaration discipline, or resolve via the existing dispatch-as-node scheduler
  by treating cross-module references as another deferred dependency.

## Dependencies

**Requires:**
- [Per-type identity for structs and methods](per-type-identity.md) — "what can a module
  export" is the load-bearing design question and types are the awkward case.

Mostly orthogonal to the effect and error work — the module loader uses whatever
`BuiltinFn` signature exists at the time. Lands cleanly any time after types settle.
