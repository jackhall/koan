""" Parse code into nested lists. Code must have string literals removed.
"""
from .common import ParseError


def first_sep(string):
    """ Given a string, return the index of the first paren (open or closed.)
        If the string contains no parens, return -1.
    """
    open = string.find('(')
    closed = string.find(')')
    return open if (closed == -1 or -1 < open < closed) else closed


def maybe_append(to_list, elem):
    """ for convenience and readability
    """
    if elem:
        to_list.append(elem)


def nest_parens(string):
    """ Takes a string that may have nested pairs of parentheses and breaks it
        into a list of str and nested lists, removing the parentheses in the
        process.

        Parameters
        ----------
        string: str

        Returns
        -------
        nested list of (str | list): the input, partitioned by parentheses
    """
    partials = [[]]  # start with a single empty expression
    while string:
        sep = first_sep(string)
        if sep == -1:  # no more parens in the string
            partials[-1].append(string)
            string = ''

        elif string[sep] == '(':
            before, string = string.split('(', maxsplit=1)
            maybe_append(partials[-1], before)
            partials.append([])  # new expression

        elif string[sep] == ')':
            inside, string = string.split(')', maxsplit=1)
            complete = partials.pop()  # current expression finished
            maybe_append(complete, inside)
            try:
                partials[-1].append(complete)
            except IndexError:  # no more partial expressions to finish
                raise ParseError('closed paren without matching open paren')

    if len(partials) > 1:  # not all partial expressions were finished
        raise ParseError('open paren without matching closed paren')
    else:
        return partials[0]
