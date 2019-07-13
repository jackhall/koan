""" Remove and reinsert string literals from code.
"""
import re

from .common import ParseError


def find_literals(quote_chars, code):
    """ Return an iterable of re.Match identifying string literals.
    """
    template = r'(?<!\\){qchar}.*?(?<!\\){qchar}'
    regex = '|'.join(template.format(qchar=q) for q in quote_chars)
    return re.finditer(regex, code)


def remove_string_literals(code):
    """ Return the code with the contents of any string literals replaced with
    integer indices. Also return a list of the literals themselves as re.Match
    objects.
    """
    literals = list(reversed(list(find_literals("'\"", code))))

    for i, match in enumerate(literals):
        start, end = match.span()
        code = code[:start+1] + str(i) + code[end-1:]

    return code, [match.group(0)[1:-1] for match in literals]


def reinsert_string_literals(code, literals):
    """ Replace the indices for string literals with the original content.
    """
    for match in reversed(list(find_literals("'\"", code))):
        start, end = match.span()
        index = int(match.group(0)[1:-1])
        code = code[:start+1] + literals[index] + code[end-1:]

    return code
