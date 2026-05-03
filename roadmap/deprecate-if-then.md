# Deprecate IF-THEN in favor of MATCH

**Problem.** [MATCH](../src/dispatch/builtins/match_case.rs) is strictly more expressive
than [IF-THEN](../src/dispatch/builtins/if_then.rs): `IF cond THEN value` is equivalent
to `MATCH cond CASE true: value`. Keeping both gives the user two equivalent constructs
to learn, keeps `if_then.rs` alive as the lone consumer of the parser's lazy-slot
machinery (`lazy_candidate` is invoked nowhere else), and forces every future branching
feature (pattern bindings, exhaustive-case checks) to be specified twice.

**Directions.** The load-bearing design call is the runtime representation of `Bool`.

- *Special-case MATCH on Bool.* Keep `KObject::Bool(bool)` as a primitive and teach MATCH
  that `true`/`false` are valid case labels for it. IF-THEN desugars to `MATCH cond CASE
  true: value` either at parse time or as a thin shim builtin. Smallest change.
- *Promote Bool to a tagged union.* `true` and `false` become the two variants of a
  built-in tagged union; MATCH dispatches over them via the same machinery as user tagged
  unions. Cleaner uniformly but changes Bool's representation
  (`KObject::Bool(bool)` → `KObject::Tagged { tag: "true"|"false", value: Null }`),
  affects every type-checking call site, and costs one `Rc` per Bool value. Worth doing
  only if other primitives are heading the same way (a bigger language-design question).
- *Hybrid.* Keep `KObject::Bool(bool)` in storage; project to a synthetic tagged union
  when MATCH consumes one. Compromise — keeps the cheap representation while letting
  MATCH treat Bool uniformly with user tagged unions.

## Dependencies

Lands cleanest after the user-defined tagged-unions substrate hardens (already shipped —
see [design/type-system.md](../design/type-system.md)), when "is Bool a tagged union" is answerable in
context. Mechanically the deprecation itself (delete `if_then.rs`, register the
desugaring, remove the lazy-slot path if nothing else needs it) is a one-PR cleanup once
the Bool question is settled.
