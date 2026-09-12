# One recognizer for a malformed keyword spine

"This keyword spine is subtly wrong" is answered once, by a shared recognizer, rather than
re-derived at each stage that cares.

**Problem.** The language answers one question — *is this keyword spine a near-miss for a builtin
form, and which mistake is it?* — through roughly twenty-five separate mechanisms spread across six
stages. [`KErrorKind::ShapeError`](../../src/machine/core/kerror.rs) is their only shared output
(~170 raise sites); there is no shared recognizer, so a new builtin form has to wire up its own
detector at every stage it wants to be diagnosed at. They group as:

- **Full-bucket-key probes.** [`form_for`](../../src/parse/forms.rs) and `key_matches`, the four
  `reserved: true` rows, [`diagnose_miss`](../../src/machine/model/miss_diagnostics.rs) and
  `key_is_reserved` at the overload write door
  ([ops.rs](../../src/machine/core/bindings/ops.rs)), and
  [`admit_bare_type_slots`](../../src/parse/forms/binder.rs).
- **A body re-inspecting a slot its own signature admitted.** `SymbolError` / `RESERVED_SYMBOLS` and
  `op_declaration_arity` ([binder.rs](../../src/parse/forms/binder.rs)), `GROUP`'s
  `check_group_context` ([group_def.rs](../../src/builtins/group_def.rs)), the `LET`
  binder-channel class check, the `FN` signature inner re-parse
  ([fn_def/signature.rs](../../src/builtins/fn_def/signature.rs)), and
  [`FormRule::Dynamic`](../../src/machine/model/close_inference.rs).
- **A scope walk.** The SIG-body refusals across nine builtins, the
  forward-reference / type-name fork, and `check_sig_group_context`.
- **Dispatch fallback.** The generic `DispatchFailed`, `quote_would_help`, and the
  keyworded-member subtyping check with its view-install replay.
- **Parse time.** Keyword-in-literal, brace-frame separators, and sigil / colon adjacency
  ([lower.rs](../../src/parse/lower.rs), [atom.rs](../../src/parse/atom.rs)).
- **Static consistency tests.** The table-shape walks, the table⟺registration law, the binder-plan
  properties, the reserved-key registration test and the lazy-kind derivation law
  ([forms/tests/](../../src/parse/forms/tests.rs)).

Nothing forces the stages to agree. A key the parse admits, dispatch refuses and the miss table has
no row for produces a generic failure; a key one stage treats as reserved and another does not is
representable.

**Acceptance criteria.**

- One recognizer answers "which builtin form is this spine a near-miss for, and how" and every
  stage above that diagnoses a near-miss reads it rather than deriving its own answer.
- Adding a builtin form registers its diagnosable mistakes in one place; no stage carries a
  hand-written case keyed by that form's spelling.
- A spine one stage recognizes and another does not is unrepresentable, or fails a consistency test.
- The diagnostic text each of today's near-miss messages produces is preserved, or the message it is
  replaced by is pinned by a test.

**Directions.**

- *Whether a diagnosable-mistake shape should be a `Form` at all — open.* Four `FORMS` rows
  (`UnaryOperatorDefinition`, `UnaryOperatorHead`, `CombinedUnaryOperator`, `CombinedLambda`) carry
  `reserved: true` and exist only to error. One position is that a form existing only to error
  should be deleted. Deleting them costs the four targeted messages, makes `key_is_reserved`
  vacuous so those keys become user-claimable, and drops three lazy stamps that hold bodies raw —
  without which an eager body can mask the real mistake. Alternatives: (a) delete the rows;
  (b) keep the reservation and the lazy stamp but drop the targeted message; (c) make the
  `-> <result>` segment grammatically optional, so `UNARY OP` is one form plus a check rather than
  a form per truncation.
- *Where the recognizer lives — open.* Candidates: a third field on a `FORMS` entry beside
  `lazy_slots`, or a `FormId`-keyed table beside `CLOSE_RULES` and `MISS_DIAGNOSTICS`.
- *How far the consolidation reaches — open.* Candidates: the full-bucket-key group alone (the
  stages that already probe `FORMS`), or every group including the scope walks and the parse-time
  adjacency rules. Recommended: the full-bucket-key group first — the parse-time rules answer a
  different question (is this text a token) and may not belong.

## Dependencies

Better done after
[round-trip the builtin forms under arbitrary layout](round-trip-builtin-forms.md), whose law over
builtin spines makes a change to near-miss recognition observable — but not blocked on it.

**Requires:** none.

**Unblocks:** none tracked yet.
