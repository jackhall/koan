# Symbol-only keyword tokens

**Problem.** A keyword part carries its spelling beside its symbol (`KeywordToken { text, symbol }`,
[src/machine/model/ast.rs](../../src/machine/model/ast.rs)), and the machine still reads the text
for logic: fixed-keyword comparisons (`kw.text() == "AS"` / `"->"` / `"_"` / `"FN"` / `"UNARY"`
in `type_decl.rs`, `branch_walk.rs`, `binder.rs`), `RESERVED_SYMBOLS`, `KeyElementSpec` matching
against the spec's spelling, operator-chain probe fragments fed to `KeywordSymbol::of_parts` as
text, and `ExpressionSignature::mint` re-homing the normalized spelling. A keyword token inside a
list or dict literal is lowered to a `KObject::KString` of its spelling (`PRINT [FOO 1]` prints
`[FOO, 1]`), which is not a language feature: a literal contains no keyword-shaped thing beyond the
structural `:` / `,` / `=` delimiters the parser consumes.

**Acceptance criteria.**

- `ExpressionPart::Keyword`, `PartClass::Keyword` and `SignatureElement::Keyword` carry a
  `KeywordSymbol` and nothing else; `KeywordToken` does not exist. Every fixed-keyword comparison
  is a symbol compare against a static name; spec matching and operator-probe minting work over
  symbols; the draft door (`SignatureElement::keyword`) interns the normalized spelling, and
  `mint` re-homes no text.
- A keyword token inside a list, dict or record literal is a parse error; the `Keyword` arms of
  `ExpressionPart::resolve` / `resolve_region_pure` are gone.
- Rendering a keyword (summaries, bucket-key diagnostics, signature text) resolves through the
  run's `LabelInterner`.
- `symbols_minted` drops on the operator-chain shape; the recorded allocation baselines do not
  regress.

**Directions.**

- *Draft spelling — decided.* `KeywordToken::drafted` (draft text vs normalized symbol) collapses
  to the normalized symbol; a pre-mint draft renders its normalized spelling.
- *Probe fragments — decided.* The chain probe and the group-registration powerset both key on a
  symbol-run digest: member symbols sorted by symbol bits, deduped, their digests hashed through
  one shared constructor, so registration and probe agree by construction and no probe path
  touches text. Registration records a rendered join under the digest for diagnostics.

## Dependencies

**Requires:** none — the interner-aware rendering seam it renders keywords through has
shipped.

**Unblocks:** none.
