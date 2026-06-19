# Files and imports

**Problem.** [`main.rs`](../../src/main.rs) reads one source string — a file path
argument or stdin — and that is the entire Koan program. There is no way for
one Koan source file to reference definitions in another: no import, no
qualified path, no project-level entry point. A Koan codebase is one file.
Realistic programs outgrow that long before they outgrow a few hundred lines,
and the language cannot represent its own standard library as separate files
because the standard library does not yet exist as Koan code at all.

The [module system](../../design/typing/modules.md) gives Koan an in-language
notion of modules — structures, signatures, and ascription — but says nothing
about how source files map onto that notion or how one file reaches into
another. This item closes that gap.

**Acceptance criteria.**

- A Koan program spanning multiple `.koan` files runs, with one file
  referencing definitions in another through the import surface.
- A `.koan` source file defines functions and types that another file uses
  via `IMPORT`, without those definitions being Rust builtins.
- An imported file exposes only the names its signature/ascription anchors as
  exports; an unexported name is not in scope at the importing site.
- A test file imports the function it tests from a separate source file and
  exercises it.

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
  [dispatch-time name placeholder](../../design/execution/name-placeholders.md#dispatch-time-name-placeholders)
  mechanism. Disallowing or requiring forward-declaration discipline is
  rejected — the scheduler already handles deferred resolution generically
  and forcing source order on multi-file projects is gratuitous.
- *Source registry per imported file — open, follow-on to source-spans.*
  Each loaded file registers a `SourceFile` via
  [`crate::source`](../../src/source.rs) so
  error frames render real `path:line:col` locations across file
  boundaries. `parse_with_path` already takes the filename; the loader
  threads it. Separately, any builtin that synthesizes AST from a literal
  `&str` at runtime (`tagged_union`, `union`, `sig_def`,
  `type_constructor`, `struct_def` — not load-bearing today) should
  register its synthetic source once via a per-builtin
  `OnceCell<FileId>` so the thread-local `SOURCES` vector doesn't grow
  unboundedly under repeated invocations. Only becomes load-bearing when
  a real builtin starts source-synthesizing in production.

## Dependencies

Soft prerequisite: lands cleanly any time after the module language is in place;
otherwise orthogonal to the effect and error work.

**Requires:** none.

**Unblocks:**

- [Standard library](standard-library.md) — the stdlib lives across
  multiple `.koan` files; user code needs an import surface to load them.
