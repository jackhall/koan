from .quotes import remove_string_literals, reinsert_string_literals


cases = {
    # no quotes
    "lorem ipsum": ("lorem ipsum",
                    []),

    # single quotes
    "''": ("'0'",
           [""]),
    "'hello'": ("'0'",
                ["hello"]),
    "'hello''bye'": ("'1''0'",
                     ['bye', 'hello']),
    "say 'hello' to me": ("say '0' to me",
                          ["hello"]),
    r"say 'hel\'lo' to me": ("say '0' to me",
                             [r"hel\'lo"]),
    "say 'bye' after 'hello'.": ("say '1' after '0'.",
                                 ["hello", "bye"]),

    # double quotes
    '""': ('"0"',
           [""]),
    '"hello"': ('"0"',
                ["hello"]),
    '"hello""bye"': ('"1""0"',
                     ['bye', 'hello']),
    'say "hello" to me': ('say "0" to me',
                          ["hello"]),
    r'say "hel\"lo" to me': ('say "0" to me',
                             [r'hel\"lo']),
    'say "bye" after "hello".': ('say "1" after "0".',
                                 ["hello", "bye"]),

    # both
    '''say 'hey "hello" you' again''': ("say '0' again",
                                        ['hey "hello" you']),
    '''say "hey 'hello' you" again''': ('say "0" again',
                                        ["hey 'hello' you"]),
    r'''say 'he\'y "hello" you' again''': ("say '0' again",
                                           [r'he\'y "hello" you']),
    r'''say 'he\'y "hel\"lo" you' again''': ("say '0' again",
                                             [r'he\'y "hel\"lo" you']),
    '''say "by'e" after 'hel"lo'.''': ('''say "1" after '0'.''',
                                       ['hel"lo', "by'e"]),
}


def test_remove():
    """ Make sure string literals can be removed properly. """
    for code, (expected_remainder, expected_literals) in cases.items():
        actual_remainder, actual_literals = remove_string_literals(code)
        assert expected_remainder == actual_remainder, 'failed on: ' + code
        assert expected_literals == actual_literals, 'failed on: ' + code


def test_replace():
    """ Make sure removing and recombining literals are inverses."""
    for code in cases:
        bare_code, literals = remove_string_literals(code)
        assert code == reinsert_string_literals(bare_code, literals)
