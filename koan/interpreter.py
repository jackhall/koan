import networkx as nx

from .parser import nest_parens


actions = [
    'map',          # functions, lists, dictionaries
    'iterate',      # can be mapped to first/rest
    'measure',      # iterates and has a known # of elements
    'contain',      # can query contents
    'mutate',       # contains and contents can be added/removed
    'order',        # can be ordered via "<" or ">"
    'equal',        # can be checked for equality (as opposed to identity)
    'summarize',    # can be mapped to a string
    'serialize',    # summarizes and can be reconstructed from string summary
    'hash',         # can be mapped reliably to a random integer
    'encode',       # can be mapped to and from a bytearray
    'execute',      # function with no arguments, expression, thread
]

# A given type may offer multiple actions. When the actions of two types are
# disjoint, their product can perform the union of actions without ambiguity.
# (The product of two types is a tuple.) Their coproduct can perform only the
# intersection of their actions. (Coproduct is an Either functor.)
# If the action sets of two types are not disjoint, their product type must
# implement that action separately (it may delegate). The product of a pair
# with third type produces a triple, not a pair with a child pair. 

# kinds of iterate: list, a map from natural numbers, generator, a set
# kinds of map: function (non-enumerable input type), dict ("small" input type)


monads = [
    'guard',        # context manager, object destructor, mutex
    'log',
    'error',
    'future',
    'state',
]


builtin_types = [  # a type is a set defined like a duck with an unit test
    'null', 'true', 'false',  # singletons
    'number',       # (numeric tower) order, equal, serialize, hash, encode
    'function',     # map
    'class',        # function, but with inheritance features
    'dict',         # function, can iterate, measure, mutate, equal(?),
                    # serialize(?), hash(?)
    'list',         # dictionary, but ordered and keys are naturals
    'relation',     # similar to a SQL table or a dataframe
    'expression',   # list of symbols and expressions, can dispatched
    'object',       # can be executed, be a node in a graph of objects
    'char',         # equal, hash, serialize, encode
    'string',       # list of char, but serializes differently
    'graph',        # mutate, equal, serialize(?)
    'module',       # graph, can execute
]


class KoanParse:
    """ Map text to tree of expressions. """
    pass


class KoanDispatch:
    """ Map an expression to an object. Has `state` monad for namespace/scope.
    """
    pass


class KoanCompile:
    """ Assemble objects into a module. Has `state` monad for graph of objects.
    Has `guard` monad for dispatcher.
    """
    pass


class KoanExecute:
    """ Walk the module until it's complete. Has a general monad for IO. """
    pass


class KoanInterpret:
    """ Walk an iterable of expressions, dispatching, incrementally compiling,
    then executing them.
    """
    pass


class KoanDebug:
    """ Walk the module, using unit tests and duck tests to find an issue. """
    pass


class KoanInterpreter:
    """ Key Ideas:
        - A program made up of expressions. Expressions are denoted by
          whitespace (newlines/spaces) or parentheses.
        - A program is a graph of module dependencies and types/morphisms.
        - An expression is a list of names and other expressions. One or more
          of the names is marked with a "`". These are keywords. Together, the
          keywords in an expression specify a function call.
        - Just like Lisp code is a data structure (a tree of expressions), Koan
          code is a graph of objects (which are defined by expressions).
        - A Koan editor may be hybrid text-graphical.
        - External duck typing. Everything that can be done with an object is
          some aspect of its type.
        - Internal typing by unit test.
        - Lexical scoping.
    """
    version = '0.0.1'

    def __init__(self):
        self.objects = nx.DiGraph()

    def nest_parens(self, string):
        return nest_parens(string)[0]
