"""Entry point for `python3 tools/modgraph <verb>`.

Run as a directory script: Python puts this package dir on `sys.path[0]`, so
the sibling modules import each other by flat name (`from score import ...`).
"""
from cli import main

if __name__ == "__main__":
    raise SystemExit(main())
