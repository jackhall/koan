# String templates

A string built from literal parts and the values between them, kept apart
until a consumer splices them.

**Problem.** No koan operation builds a string from others: nothing
concatenates two strings or writes a value into one, and `+` over strings reads
badly enough that Python and JavaScript programs build strings with templates
instead. JavaScript's tagged templates and Python's t-strings keep a template's
literal parts and its interpolated values apart until a consumer, such as an
SQL escaper, splices them. That is the shape of a quote with its bindings
([quotes](../../src/scope/README.md#quotes)).

**Acceptance criteria.**

- A template keeps its literal parts and its values apart, and a consumer reads
  each.
- Splicing a template is a view over its parts
  ([views](../../src/values/README.md#views)), resolved at a crossing as every
  view is.
- The plain splice renders each value as `PRINT` renders it.

**Directions.**

- *A template is a quote — open.* Whether a template is a quote whose code is
  a string literal naming `$` values, so that `USING` fills it and a consumer
  reads it as it reads any code, or a value of a kind of its own.
- *Where a value enters a literal — open.* Koan spends `$` and `\` on a quote's
  bindings and marks, and `..` on spread.
- *`JOIN` — open.* A template has a fixed arity, so it cannot build a string
  from a list of strings; a `JOIN` builtin over a list, as Python's `str.join`
  is, would, as a view.

## Dependencies

**Requires:**

- [Views](../../src/values/README.md#views) — shipped: the string views a
  template's splice resolves to.

**Unblocks:** none — a leaf.
