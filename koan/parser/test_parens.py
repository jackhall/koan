import unittest

from .parens import nest_parens, ParseError


class TestNestParens(unittest.TestCase):
    def diff_nested_parens(self, x_list, y_list):
        self.assertEqual(len(x_list), len(y_list))

        for x, y in zip(x_list, y_list):
            self.assertEqual(type(x), type(y))
            if isinstance(x, str):
                self.assertEqual(x, y)
            elif isinstance(x, list):
                self.diff_nested_parens(x, y)
            else:
                self.fail('a `nest_parens` structure is made of list and str')

    def test_with_good_lines(self):
        """ Make sure valid pairs of nested parens can be matched.
        """
        good_lines = {
            '': [],
            '()': [[]],
            '(hi there)': [['hi there']],
            'hey (whoever you are) look  at':
                ['hey ', ['whoever you are'], ' look  at'],
            'hey (whoever you are) look at (that over there)':
                ['hey ', ['whoever you are'], ' look at ', ['that over there']],
            'hey (whoever you are) look at (whatever (that over there) is)':
                ['hey ', ['whoever you are'], ' look at ',
                 ['whatever ', ['that over there'], ' is']],
            'hey (whoever you are)(hello in this language)':
                ['hey ', ['whoever you are'], ['hello in this language']],
            'hey (whoever (I think) you are (when I remember) now) look at':
                ['hey ', ['whoever ', ['I think'], ' you are ',
                          ['when I remember'], ' now'], ' look at']
        }

        for line, answer in good_lines.items():
            with self.subTest(version=nest_parens.__name__, test_line=line):
                result = nest_parens(line)
                self.diff_nested_parens(answer, result)

    def test_with_bad_lines(self):
        """ Make sure invalid pairs of nested parens raise an exception.
        """
        bad_lines = [
            ')(',
            'has (open paren only',
            'has closed) paren only',
            '(two (open one closed)',
            'two (closed one) open)',
        ]
        for line in bad_lines:
            with self.subTest(version=nest_parens.__name__, test_line=line):
                with self.assertRaises(ParseError):
                    nest_parens(line)


if __name__ == '__main__':
    unittest.main()
