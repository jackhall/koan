"""
- identify and temporarily remove string literals
    - identify string literals (regex)
    - collapse code into a single line (ignore newlines in quotes) (regex)
"""
import re

from .common import ParseError


def remove_string_literals(code):
    """ Remove the contents of all string literals from code.
    """
    quotes = re.compile(r"""(?<!\\)".*?(?<!\\)"|(?<!\\)'.*?(?<!\\)'""")
    matches = list(re.finditer(quotes, code))

    remainder = code
    for match in reversed(matches):
        i, j = match.span()
        remainder = remainder[:i+1] + remainder[j-1:]

    # detect non-doubled quotes in remainder?

    return remainder, matches
