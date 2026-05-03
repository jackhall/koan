# Refactor for cleaner abstractions

**Standing item, exploratory.** The other roadmap entries add features; this one's job is
to *remove* — places where the abstraction grew accidentally and a generalization has
become visible. Examples worth a look when surrounding code next changes for unrelated
reasons:

- *Builtin registration patterns.* The `register_builtin` + signature-construction
  skeleton repeats across [builtins/](../src/dispatch/builtins/). Whether the duplication
  is noise to factor or "deliberate so each builtin reads top-to-bottom on its own" is
  an open call — the answer depends on how builtins evolve under monadic effects and
  user-defined types.
- *Parser pass boundaries.* [parse/](../src/parse/)'s passes pipe strings between each
  other (`quotes.rs` → `whitespace.rs` → `expression_tree.rs`). Typed outputs would
  compose more cleanly. Low priority — current pipeline works.

**When to act.** Refactor each only when the next feature would multiply the existing
duplication. Don't refactor preemptively; the cost of churn outweighs the cost of
carrying a small duplication that hasn't grown teeth yet.

## Dependencies

Standing/exploratory — no fixed prerequisites.
