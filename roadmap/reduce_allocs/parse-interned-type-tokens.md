# Parse-interned type tokens

**Problem.** The parser classifies a Type token (`classify_atom`,
[src/parse/tokens.rs](../../src/parse/tokens.rs)) and carries it as bare text —
`ExpressionPart::Type(TypeIdentifier<'a>)`, a newtype over `&'a str`
([src/machine/model/ast.rs](../../src/machine/model/ast.rs)). Every downstream seam re-derives
the `TypeSymbol` from that text at runtime: the type-lookup ladder classifies at its top
(`resolve_type_with_chain`, [src/machine/core/scope/resolve.rs](../../src/machine/core/scope/resolve.rs)),
`elaborate_type_identifier` mints twice per leaf, variant-tag matches hash the token
(`branch_walk.rs`, `apply_callable.rs`, `typed_field_list.rs` — one site mints the same token
twice), and every type-declaring builtin mints its binder name through `type_binder` /
`require_bare_type_name` after rendering the token to a `String`. The text also has to be kept
alive: `Held::UnresolvedType`, `Carried::UnresolvedType`, `DeferredReturn::Type`,
`TypeCapture::AtWake`, `ReturnTypeCapture::Unresolved(String)` and friends all carry the name's
bytes, so a type name crossing a region boundary is re-bumped (`lift.rs`, `reach.rs`'s
`AdoptedType`) and a deferred return type is rebuilt from a `String` at finish
(`fn_def/return_type.rs`). `KType::from_name`
([src/machine/model/types/ktype_resolution.rs](../../src/machine/model/types/ktype_resolution.rs))
matches the eleven builtin type names by string compare at every bind-seam and elaboration
fall-through. Keyword tokens already carry their symbol from the parse boundary; Type tokens do
not.

**Acceptance criteria.**

- `ExpressionPart::Type` carries a `TypeSymbol` and nothing else; the parser mints and interns it
  where it classifies the token. `TypeIdentifier` does not exist; every carrier that held one
  (`Held` / `Carried::UnresolvedType`, `FieldSlot::Type`, `DeferredReturn::Type`,
  `DeferredReturnSurface::Type`, `TypeCapture::AtWake`, `CarrierForm`, `ReturnTypeRaw`,
  `ReturnTypeState`, `ReturnTypeCapture`) holds the `TypeSymbol` and is lifetime-free.
- The type-side lookup ladder and `elaborate_type_identifier` take a `TypeSymbol`; variant-tag and
  member matches compare carried symbol bits; `require_bare_type_name` yields a `TypeSymbol`; the
  type-side binder-name extractors yield a `TypeSymbol`; the type-declaring builtins read the carried
  symbol. `TypeSymbol::of(text)` is deleted — `declared` is its only constructor, and its production
  callers are the parser, builtin registration, and the rendered-builtin-name sites
  (`val_decl.rs`, `let_binding.rs`, `require_bare_type_name`'s `Held::Type` arm).
- `KType::from_symbol(TypeSymbol)` is the builtin type lookup: the eleven builtin type names are
  declared as `StaticName<TypeSymbol>`s and the table compares symbol bits, so no seam classifies
  a builtin type name from text. `KType::from_name(&str)` is gone, and `builtins::builtin_type_name`
  registers each name off its static.
- Rendering a part or expression (`summarize`, trace frames, diagnostics that name a token)
  resolves the symbol through the run's `LabelInterner`; a miss renders the standard placeholder.
  The type-name re-bump sites and the deferred-return rebuild are gone.
- `symbols_minted` drops on every shape that names a type; the recorded allocation baselines do
  not regress; `tests/allocation_baseline.rs` stays green.

**Directions.**

- *Rendering seam — decided.* `Part::summarize` and the expression `summarize`s, `summarize_parts`,
  `spliced_summary`, and `TraceFrame::from_expr` take `&LabelInterner`; `DictFrame` and
  `parse_pair_list` get it threaded. Lands here because this is the first symbol-only part.
- *Deferred-return digest — decided.* `type_digest.rs` feeds the `TypeSymbol` bits for
  `DeferredReturnSurface::Type`; nothing persists digests across runs.
- *`resolve_name`'s value-channel probe of a Type token — decided.* Deleted: `ValueSymbol::of`
  of Type-class text is `None`, so the probe is a guaranteed miss today.

## Dependencies

**Requires:** none — the `symbols_minted` figure this item quotes is shipped.

**Unblocks:**

- [Parse-interned identifiers](parse-interned-identifiers.md) — the value-side mirror, reusing the
  rendering seam and ladder shape.
- [Symbol-only keyword tokens](symbol-only-keyword-tokens.md) — renders keywords through the same
  interner-aware seam.
