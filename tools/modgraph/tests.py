"""Regression tests for the modgraph what-if rewriter.

Run directly — the script's own directory lands on `sys.path[0]`, so the flat
package imports (`from rewrite import ...`) resolve exactly as they do under
`python3 tools/modgraph <verb>`:

    python3 tools/modgraph/tests.py

The headline coverage is facade-aware item moves: the canonical DOT attributes a
re-exported import to the `pub use` facade, so an item move's edge *removal* must
target that facade, not the symbol's def-site. The fixture writes a tiny `src/`
tree with a facade re-export and three consumers (facade-routed, mixed, deep) and
asserts the `(add, remove)` delta and the rendered DOT.
"""
from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import reexport
from graph import parse_dot, write_dot
from rewrite import MoveSpec, diff_edges, render_dot_diff

# Synthetic SCIP symbols — opaque strings; the only contract is that a consumer's
# `refs` entry equals the move's `.symbol` so diff_edges sees the reference.
SCOPE_ID_SYM = "scip ScopeId"
OTHER_SYM = "scip OtherThing"

# The module node set the canonical (re-export-corrected) DOT would carry. Note
# `machine::core` (the facade file) and `machine::core::scope_id` (where the
# items are defined) coexist — Rust-2018 file+dir module style.
KNOWN = {
    "koan::machine",
    "koan::machine::core",
    "koan::machine::core::scope_id",
    "koan::machine::consumer_facade",
    "koan::machine::consumer_mixed",
    "koan::machine::consumer_deep",
}

# What each consumer file imports. core.rs re-exports ScopeId/OtherThing from its
# scope_id child; consumers reach them through the `machine::core` facade except
# `consumer_deep`, which names the def-site path directly.
SRC_FILES = {
    "machine/core.rs":
        "pub use self::scope_id::ScopeId;\npub use self::scope_id::OtherThing;\n",
    "machine/core/scope_id.rs":
        "pub struct ScopeId;\npub struct OtherThing;\n",
    "machine/consumer_facade.rs":
        "use crate::machine::core::ScopeId;\n",
    "machine/consumer_mixed.rs":
        "use crate::machine::core::{ScopeId, OtherThing};\n",
    "machine/consumer_deep.rs":
        "use crate::machine::core::scope_id::ScopeId;\n",
}

# Consumers reference the moved symbol (and, for the mixed consumer, a second
# symbol that stays put). diff_edges reads only `refs`.
DOCS = {
    "src/machine/consumer_facade.rs": {"defs": [], "refs": [(SCOPE_ID_SYM, 0)]},
    "src/machine/consumer_mixed.rs":
        {"defs": [], "refs": [(SCOPE_ID_SYM, 0), (OTHER_SYM, 0)]},
    "src/machine/consumer_deep.rs": {"defs": [], "refs": [(SCOPE_ID_SYM, 0)]},
    "src/machine/core/scope_id.rs":
        {"defs": [(SCOPE_ID_SYM, 0), (OTHER_SYM, 0)], "refs": []},
}

FACADE = "koan::machine::core"
DEF_SITE = "koan::machine::core::scope_id"
NEW_MOD = "koan::machine::scope"
C_FACADE = "koan::machine::consumer_facade"
C_MIXED = "koan::machine::consumer_mixed"
C_DEEP = "koan::machine::consumer_deep"


def _scope_id_move() -> MoveSpec:
    mv = MoveSpec(old_path=f"{DEF_SITE}::ScopeId", new_module=NEW_MOD)
    mv.symbol = SCOPE_ID_SYM
    mv.source_module = DEF_SITE
    mv.source_file = "src/machine/core/scope_id.rs"
    return mv


def _write_src(root: Path) -> None:
    for rel, body in SRC_FILES.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body, encoding="utf-8")


class FacadeAwareItemMove(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.src_root = Path(self._tmp.name) / "src"
        _write_src(self.src_root)
        self.attribution = reexport.attributions(KNOWN, self.src_root, "koan")

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def test_attribution_routes_through_facade(self):
        """reexport.attributions charges the facade import to the facade, not
        the def-site — the asymmetry the removal logic must account for."""
        self.assertIn((FACADE, "ScopeId"), self.attribution[C_FACADE])
        self.assertCountEqual(
            self.attribution[C_MIXED],
            [(FACADE, "ScopeId"), (FACADE, "OtherThing")],
        )
        self.assertIn((DEF_SITE, "ScopeId"), self.attribution[C_DEEP])

    def test_diff_edges_facade_aware(self):
        add, remove = diff_edges(DOCS, [_scope_id_move()], self.attribution)

        # Every consumer that referenced ScopeId now points at the new module.
        self.assertEqual(
            add, {(C_FACADE, NEW_MOD), (C_MIXED, NEW_MOD), (C_DEEP, NEW_MOD)}
        )
        # Removal drops the *facade* edge for the facade-only consumer (not the
        # def-site), drops the def-site edge for the deep consumer, and keeps the
        # facade edge for the mixed consumer (it still imports OtherThing there).
        self.assertEqual(
            remove, {(C_FACADE, FACADE), (C_DEEP, DEF_SITE)}
        )
        # The bug guard: the def-site removal that used to silently no-op must
        # NOT be emitted for the facade-routed consumer.
        self.assertNotIn((C_FACADE, DEF_SITE), remove)

    def test_rendered_dot_has_no_double_edge(self):
        """End-to-end: no consumer points at both the stale facade and the new
        module after the pass (roadmap acceptance criterion 2)."""
        owns = [
            ("koan::machine", "koan::machine::core"),
            ("koan::machine::core", DEF_SITE),
            ("koan::machine", C_FACADE),
            ("koan::machine", C_MIXED),
            ("koan::machine", C_DEEP),
        ]
        uses = [(C_FACADE, FACADE), (C_MIXED, FACADE), (C_DEEP, DEF_SITE)]
        dot_path = self.src_root.parent / "modules.dot"
        write_dot(dot_path, KNOWN, owns, uses)
        original = dot_path.read_text(encoding="utf-8")

        add, remove = diff_edges(DOCS, [_scope_id_move()], self.attribution)
        rendered = render_dot_diff(original, add, remove, {NEW_MOD})

        out_path = self.src_root.parent / "rewritten.dot"
        out_path.write_text(rendered, encoding="utf-8")
        result = {tuple(e) for e in parse_dot(out_path).uses}

        # Facade-only consumer: redirected, facade edge gone — not pointing at both.
        self.assertIn((C_FACADE, NEW_MOD), result)
        self.assertNotIn((C_FACADE, FACADE), result)
        # Mixed consumer: gains the new edge, keeps the facade (OtherThing stays).
        self.assertIn((C_MIXED, NEW_MOD), result)
        self.assertIn((C_MIXED, FACADE), result)
        # Deep consumer: redirected, def-site edge gone.
        self.assertIn((C_DEEP, NEW_MOD), result)
        self.assertNotIn((C_DEEP, DEF_SITE), result)


if __name__ == "__main__":
    unittest.main()
