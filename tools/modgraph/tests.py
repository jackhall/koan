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

`OutboundAwareItemMove` covers the symmetric outbound side: the dependency edges a
moved item carries in its own body. A two-item group leaving one source module
must reconstitute its members' outbound edges at the new module (facade-corrected,
unioned), drop a reference between two group members, and retract a source edge no
surviving item still justifies.
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
        add, remove = diff_edges(DOCS, [_scope_id_move()], self.attribution, KNOWN)

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

        add, remove = diff_edges(DOCS, [_scope_id_move()], self.attribution, KNOWN)
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


# ---------------------------------------------------------------------------
# Outbound side: the edges a moved item carries in its own body.
# ---------------------------------------------------------------------------

# Two items, Alpha and Beta, leave `machine::core` together for `machine::handles`.
# Alpha's body references Widget (imported through the `machine::model` facade) and
# its moving sibling Beta; Beta's body references Gadget (deep-imported from
# `model::internal`). Gamma stays in core and also references Widget.
OB_KNOWN = {
    "koan::machine",
    "koan::machine::core",
    "koan::machine::model",
    "koan::machine::model::internal",
}

# core.rs imports Widget through the model facade and Gadget through its def-site.
OB_SRC_FILES = {
    "machine/core.rs":
        "use crate::machine::model::Widget;\n"
        "use crate::machine::model::internal::Gadget;\n",
}

WIDGET_SYM = "rust-analyzer cargo koan 0.1.0 machine/model/internal/Widget#"
GADGET_SYM = "rust-analyzer cargo koan 0.1.0 machine/model/internal/Gadget#"
ALPHA_SYM = "rust-analyzer cargo koan 0.1.0 machine/core/Alpha#"
BETA_SYM = "rust-analyzer cargo koan 0.1.0 machine/core/Beta#"
GAMMA_SYM = "rust-analyzer cargo koan 0.1.0 machine/core/Gamma#"
# An external-crate ref the resolver must drop rather than attach to the koan root.
STD_SYM = "rust-analyzer cargo std 1.0.0 vec/Vec#"

# core.rs body layout (0-indexed lines):
#   3 `struct Alpha {`  4 `w: Widget,`  5 `b: Beta,`  6 `v: Vec<..>,`  7 `}`
#   8 `struct Beta {`   9 `g: Gadget,`  10 `}`
#  11 `struct Gamma {`  12 `w: Widget,`  13 `}`
OB_DOCS = {
    "src/machine/core.rs": {
        "defs": [(ALPHA_SYM, 3), (BETA_SYM, 8), (GAMMA_SYM, 11)],
        "refs": [
            (WIDGET_SYM, 4), (BETA_SYM, 5), (STD_SYM, 6),
            (GADGET_SYM, 9),
            (WIDGET_SYM, 12),
        ],
    },
}

OB_CORE = "koan::machine::core"
OB_MODEL = "koan::machine::model"
OB_INTERNAL = "koan::machine::model::internal"
OB_HANDLES = "koan::machine::handles"


def _ob_move(symbol: str, name: str, def_line: int, body_end: int) -> MoveSpec:
    mv = MoveSpec(old_path=f"{OB_CORE}::{name}", new_module=OB_HANDLES)
    mv.symbol = symbol
    mv.source_module = OB_CORE
    mv.source_file = "src/machine/core.rs"
    mv.def_line = def_line
    mv.body_end = body_end
    return mv


class OutboundAwareItemMove(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.src_root = Path(self._tmp.name) / "src"
        for rel, body in OB_SRC_FILES.items():
            path = self.src_root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body, encoding="utf-8")
        self.attribution = reexport.attributions(OB_KNOWN, self.src_root, "koan")
        self.moves = [
            _ob_move(ALPHA_SYM, "Alpha", def_line=3, body_end=7),
            _ob_move(BETA_SYM, "Beta", def_line=8, body_end=10),
        ]

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def test_attribution_separates_facade_from_deep(self):
        """core enters Widget through the model facade, Gadget through its
        def-site — the two attribution modes the outbound resolver must honor."""
        self.assertCountEqual(
            self.attribution[OB_CORE],
            [(OB_MODEL, "Widget"), (OB_INTERNAL, "Gadget")],
        )

    def test_outbound_add_reconstitutes_back_edges(self):
        add, _remove = diff_edges(OB_DOCS, self.moves, self.attribution, OB_KNOWN)
        # Each member's outbound dep reappears at the new module, facade-corrected
        # and unioned across the group — the back-edges today's tool omits.
        self.assertIn((OB_HANDLES, OB_MODEL), add)       # Alpha -> Widget (facade)
        self.assertIn((OB_HANDLES, OB_INTERNAL), add)    # Beta  -> Gadget (deep)

    def test_outbound_intra_group_ref_drops(self):
        add, _remove = diff_edges(OB_DOCS, self.moves, self.attribution, OB_KNOWN)
        # Alpha -> Beta is a reference between two members of the same group: it
        # collapses to a handles -> handles self-edge and is dropped, and never
        # leaks back as a spurious source-module inbound edge either.
        self.assertNotIn((OB_HANDLES, OB_HANDLES), add)
        self.assertNotIn((OB_CORE, OB_HANDLES), add)

    def test_outbound_external_ref_dropped(self):
        add, _remove = diff_edges(OB_DOCS, self.moves, self.attribution, OB_KNOWN)
        # The `std::vec::Vec` ref resolves to no koan module — no edge to the root.
        self.assertNotIn((OB_HANDLES, "koan"), add)
        self.assertEqual(add, {(OB_HANDLES, OB_MODEL), (OB_HANDLES, OB_INTERNAL)})

    def test_outbound_remove_only_when_unjustified(self):
        _add, remove = diff_edges(OB_DOCS, self.moves, self.attribution, OB_KNOWN)
        # Gadget was referenced only from the departing Beta, so core -> internal
        # is retracted; Widget still lives in surviving Gamma, so core -> model
        # stays.
        self.assertIn((OB_CORE, OB_INTERNAL), remove)
        self.assertNotIn((OB_CORE, OB_MODEL), remove)


if __name__ == "__main__":
    unittest.main()
