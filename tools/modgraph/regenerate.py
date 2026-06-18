"""Regenerate the source-data files the scorer reads from.

The canonical module graph is built in three steps: run `cargo modules
dependencies` for the raw structure, **re-attribute its `uses` edges to the
import surface the author wrote** (`reexport.correct` — core construction, not
an option), and write the corrected `observe/modules.dot`. The companion
`observe/doc_graph.dot` is refreshed via `tools/doclinks.py signals` so the
source and doc graphs reflect the same working tree. Steps fail loudly — a
silent stale DOT is the bug this exists to prevent.
"""
from __future__ import annotations

import subprocess
from pathlib import Path

import reexport
from graph import parse_dot, write_dot

REPO = Path(__file__).resolve().parents[2]


def regenerate_source_data(
    edges_path: Path,
    src_root: Path = Path("src"),
    package: str = "koan",
    reexport_correct: bool = True,
) -> None:
    cargo_cmd = [
        "cargo", "modules", "dependencies",
        "--package", package, "--lib",
        "--no-externs", "--no-sysroot",
        "--no-traits", "--no-fns", "--no-types",
    ]
    print(f"regenerating {edges_path} via `cargo modules dependencies`...")
    proc = subprocess.run(cargo_cmd, cwd=REPO, capture_output=True, text=True, check=False)
    if proc.returncode != 0:
        raise SystemExit(
            f"cargo modules failed (exit {proc.returncode}):\n{proc.stderr}"
        )
    edges_path.write_text(proc.stdout)

    if reexport_correct:
        print("re-attributing uses edges to written import surface "
              "(reexport correction)...")
        graph = parse_dot(edges_path)
        corrected = reexport.correct(graph.nodes, src_root, package)
        write_dot(edges_path, graph.nodes, graph.owns, corrected)

    doclinks = REPO / "tools" / "doclinks.py"
    print("regenerating observe/doc_graph.dot via `doclinks.py signals`...")
    proc = subprocess.run(
        ["python3", str(doclinks), "signals"],
        cwd=REPO, capture_output=True, text=True, check=False,
    )
    if proc.returncode != 0:
        raise SystemExit(
            f"doclinks signals failed (exit {proc.returncode}):\n{proc.stderr}"
        )
