import unittest

from .quotes import remove_string_literals


class TestNoLiterals(unittest.TestCase):
    cases = {
        # no quotes
        "lorem ipsum": ("lorem ipsum",
                        []),

        # single quotes
        "''": ("''",
               [""]),
        "'hello'": ("''",
                    ["hello"]),
        "say 'hello' to me": ("say '' to me",
                              ["hello"]),
        r"say 'hel\'lo' to me": ("say '' to me",
                                 [r"hel\'lo"]),
        "say 'bye' after 'hello'.": ("say '' after ''.",
                                     ["bye", "hello"]),

        # double quotes
        '""': ('""',
               [""]),
        '"hello"': ('""',
                    ["hello"]),
        'say "hello" to me': ('say "" to me',
                              ["hello"]),
        r'say "hel\"lo" to me': ('say "" to me',
                                 [r'hel\"lo']),
        'say "bye" after "hello".': ('say "" after "".',
                                     ["bye", "hello"]),

        # both
        '''say 'hey "hello" you' again''': ('''say '' again''',
                                            ['hey "hello" you']),
        '''say "hey 'hello' you" again''': ('''say "" again''',
                                            ["hey 'hello' you"]),
        r'''say 'he\'y "hello" you' again''': ('''say '' again''',
                                            [r'he\'y "hello" you']),
        r'''say 'he\'y "hel\"lo" you' again''': ('''say '' again''',
                                            [r'he\'y "hel\"lo" you']),
    }

    def test_remove(self):
        """ Make sure string literals can be removed properly. """
        for code, (expected_remainder, expected_literals) in cases.items():
            actual_remainder, matches = remove_string_literals(code)
            self.assertEqual(expected_remainder, actual_remainder)

            actual_literals = [m.group(0)[1:-1] for m in matches]
            self.assertEqual(expected_literals, actual_literals)

    def test_replace(self):
        """ Make sure removing and recombining literals are inverses."""
        pass
