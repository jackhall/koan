# Eliminate SIG's dual-write

Give a signature declaration a single type-namespace home, retiring the
last `(bindings.types, bindings.data)` dual-write the way every other
nominal binder already lands.

**Problem.** A SIG declaration is the sole surviving caller of
[`Scope::register_nominal`](../../src/machine/core/scope.rs): it writes
`types[S] = SatisfiesSignature { sig_id, sig_path, … }` (the constraint
form a `:S` annotation means) alongside `data[S] = KTypeValue(Signature)`
(the value form [`:|`](../../src/builtins/ascribe.rs) and `SIG_WITH`
introspect, carrying a live `decl_scope` pointer). The two entries hold
genuinely different content, so — unlike STRUCT / UNION / MODULE — the
value side can't be synthesised from the type side:
[`coerce_type_token_value`](../../src/machine/execute/dispatch/resolve_type_expr.rs)
would reconstruct the constraint, not the `Signature`. The dual-write,
and the `register_nominal` / `try_register_nominal` machinery backing it,
persist solely for this case.

**Impact.**

- `register_nominal` / `try_register_nominal` and the atomic `(types, data)`
  write delete entirely; no nominal binder dual-writes.
- `bindings.data` carries zero type carriers — the type-language /
  value-language partition becomes total.

**Directions.**

- **Type side stores the `Signature` value — open.** Candidate: store
  `types[S] = KType::Signature(s)` (which carries `decl_scope`, so `:|`
  synthesises correctly) and convert `Signature → SatisfiesSignature` at
  every `:S` annotation-elaboration site; the conversion is total
  (`sig_id` and `path` derive from the `Signature`). Risk lives in the
  type-annotation path. Recommended pending a survey of every
  `SatisfiesSignature` producer.

## Dependencies

**Requires:** none.

**Unblocks:** none.
