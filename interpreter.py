import networkx as nx


actions = [
    'map',          # functions, lists, dictionaries
    'iterate',      # can be looped over, unpacked into "first" and "rest"
    'measure',      # iterates and has a known # of elements
    'contain',      # can query contents
    'mutate',       # contains and contents can be added/removed
    'order',        # can be ordered via "<" or ">"
    'equal',        # can be checked for equality (as opposed to identity)
    'summarize',    # can be mapped to a string
    'serialize',    # summarizes and can be reconstructed from string summary
    'hash',         # can be mapped reliably to an integer
    'encode',       # can be mapped to and from a bytearray
    'guard',        # context manager, object destructor, mutex
    'execute',      # function with no arguments, expression, thread
]


builtin_types = [
    'null', 'true', 'false',  # singletons
    'number',       # (numeric tower) order, equal, serialize, hash, encode
    'function',     # map
    'class',        # function, but with inheritance features
    'dict',         # function, can iterate, measure, mutate, equal(?),
                    # serialize(?), hash(?)
    'list',         # dictionary, but ordered and keys are naturals
    'expression',   # list, can execute
    'char',         # equal, hash, serialize, encode
    'string',       # list of char, but serializes differently
    'graph',        # mutate, equal, serialize(?)
    'module',       # graph, can execute
    'thread',       # module
]


class KoanObject:
    def __init__(self, ktype):
        self.ktype = ktype


def next_paren(string):
    open = string.find('(')
    closed = string.find(')')
    return open if open < closed else -1


def close_paren(string):
    try:
        inside, after = string.split(')', maxsplit=1)
        return inside, after
    except ValueError:  # if tuple unpacking fails
        return string, ''


def nest_parens(string):
    expr = []
    open = next_paren(string)
    while open != -1:  # while there are more subexpressions
        if open > 0:
            expr.append(string[:open])
        # expr.extend(string[:open].split())
        inside, string = nest_parens(string[open+1:])
        expr.append(inside)
        open = next_paren(string)

    if string:  # if the string did end on a closed paren
        inside, after = close_paren(string)
        expr.append(inside)
        # expr.extend(inside.split())
    else:
        after = ''
    return expr, after


class KoanInterpreter:
    """ Key Ideas:
        - A program is a graph described by expressions. Expressions are
          denoted by whitespace (newlines/spaces) or parentheses.
        - An expression is a list of names and other expressions. One or more
          of the names is marked with a "`". These are keywords. Together, the
          keywords in an expression specify a function call.
        - Just like Lisp code is a data structure (a tree of expressions), Koan
          code is a graph of objects (some of which are expressions?).
        - A Koan editor may be hybrid text-graphical.
        - Duck typing. Everything that can be done with an object (including
          calling it as a function) is just another aspect of its type.
        - Lexical scoping.
    """
    version = '0.0.1'

    def __init__(self):
        self.objects = nx.DiGraph()

    def nest_parens(self, string):
        return nest_parens(string)[0]


test_lines = [
    '(hi there)',  # extra spaces before and after
    'hey (whoever you are) look  at',  # careful to avoid the empty term
    'hey (whoever you are) look at (that over there)',
    'hey (whoever you are) look at (whatever (that over there) is)',  # fails
]
