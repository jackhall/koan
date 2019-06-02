import networkx as nx

from .parser import nest_parens


# core: needed to implement Parser, Dispatcher, Compiler and Executer
# library: vital for writing good general-purpse code


core_actions = [
    'map',          # functions, lists, dictionaries
    'iterate',      # can be mapped to first/rest (axiom of choice for sets?)
    'equal',        # can be checked for equality (as opposed to identity)
    'contain',      # can query contents
    'hash',         # can be mapped reliably to a random integer
    'encode',       # can be mapped to and from a bytearray
    'execute',      # function with no arguments, expression, thread
]

library_actions = [
    'measure',      # iterates and has a known # of elements
    'mutate',       # contains and contents can be added/removed (state monad)
    'order',        # can be ordered via "<" or ">"
    'summarize',    # map to a string
    'serialize',    # summarizes and can be reconstructed from string summary
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


core_monads = [
    'guard',        # context manager, object destructor, mutex
    'error',
    'state',
]


library_monads = [
    'io',
    'log',
    'future',
]


core_types = [  # a type is a set defined like a duck with an unit test
    'null', 'true', 'false',  # singletons
    'number',       # (numeric tower) order, equal, serialize, hash, encode
    'function',     # map
    'dict',         # function, can iterate, measure, mutate, equal(?),
                    # serialize(?), hash(?)
    'object',       # can be executed, be a node in a graph of objects
    'list',         # dictionary, but ordered and keys are naturals
    'expression',   # list of symbols and expressions, can dispatched
    'char',         # equal, hash, serialize, encode
    'string',       # list of char, but serializes differently
    'graph',        # mutate, equal, serialize(?)
    'module',       # graph of objects, can be executed
]


library_types = [
    'class',        # function creating types, with features for extension,
                    # product and coproduct
    'relation',     # similar to a SQL table or a dataframe
]


class Object:
    def __init__(self, unbound_kwargs):
        self.positional_args = None  # [] indicates a placeholder
        self.remaining_args = unbound_kwargs

    def execute(self, *args, **kwargs):
        """ If all args are present, execute. Else, return new bound Object.
        All keyword args must be present, and at least one positional arg. It
        may be necessary to define a placeholder to handle no positional args.
        If not executable, return self? Like a map from singleton?
        """
        pass


class Expression(list):
    """ Object that represents code. """
    def __init__(self, parent):
        self.parents = {parent}

    def __eq__(self, other):
        """ Can this expression be substituted for another? """
        pass

    def __hash__(self):
        """ Based on hashes of elements, for checking existing expressions. """
        pass

    def __getstate__(self):
        """ For pickling. """
        pass

    def __setstate__(self):
        """ For unpickling. """
        pass


class Parser:
    """ Map text to tree of expressions. """
    def parse(self, code):
        pass


class Dispatcher:
    """ Walk tree of expressions, map expressions to a morphism or type.
    Has `state` monad for namespace/scope.
    """
    def __init__(self, scope):
        self.namespace = scope

    def dispatch(self, expression):
        # May recognize the expression refers to an existing object.
        pass


class Module:
    """ Object with entry point for each executable object, a set of types/
    checkers, and properties of the category for help executing.

    State here would mean the module itself can be changed on the fly.
    Otherwise the non-executable objects are types. Is a module a category? To
    deal with state or similar, the module would have to offer a monad.

    Need a way to specify/test properties of objects and combinations thereof,
    for instance inversion, idempotence or associativity.
    """
    pass


class Compiler:
    """ Assemble objects into a module. Has `state` monad for graph of objects.
    Has `guard` monad for dispatcher.
    """
    pass


class Executer:
    """ Walk the module and any dependent modules until it's complete.
    Has a general monad for IO and a state monad for instance state.
    """
    pass


class Interpreter:
    """ Walk an iterable of expressions, dispatching, incrementally compiling,
    then executing them.
    """
    pass


class Debugger:
    """ Walk the module, using unit tests and duck tests to find an issue. """
    pass


class KoanInterpreter:  # just a bunch of notes now
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
