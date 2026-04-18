""" Collapse syntactically-significant whitespace using parentheses. This will
always be straightforward if all parens are closed on the same line. First
parse each line into an expression, then build those into expressions using the
whitespace.
"""
import re

from .common import ParseError


_leading_spaces = re.compile('^ *')


def strip_indent(line):
    match = _leading_spaces.match(line)
    indent = 0 if match is None else (match.end - match.start)
    return indent, line[match.end:]


def collapse_lines(code):
    if '\t' in code:
        raise ParseError('tab characters not allowed outside string literals')

    lines = code.split('\n')
    current_indent, collapsed = strip_indent(lines[0])
    for line in lines[1:]:

        # group lines by leading whitespace
        # an increase in indentation denotes continuation of an expression
        pass
