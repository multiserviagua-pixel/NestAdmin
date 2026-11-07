import importlib.util
from pathlib import Path
import unittest

MODULE = Path(__file__).resolve().parents[1] / 'renumerar.py'
spec = importlib.util.spec_from_file_location('renumerar', MODULE)
renumerar = importlib.util.module_from_spec(spec)
spec.loader.exec_module(renumerar)


class RenumerarTests(unittest.TestCase):
    def test_ranges_are_expanded(self):
        self.assertEqual(renumerar.parse_mapping(['D92-D94=D106', 'D99=D113']),
                         {92: 106, 93: 107, 94: 108, 99: 113})

    def test_substitution_is_simultaneous(self):
        # D92→D100 y D100→D114 a la vez: el D100 nuevo no vuelve a cambiar.
        text, count = renumerar.renumber('D92 y D100', {92: 100, 100: 114})
        self.assertEqual((text, count), ('D100 y D114', 2))

    def test_only_whole_words(self):
        text, count = renumerar.renumber('DV-1, 3D92, D920, AD92, `D92`', {92: 106})
        self.assertEqual(text, 'DV-1, 3D92, D920, AD92, `D106`')
        self.assertEqual(count, 1)

    def test_single_digit_keeps_zero(self):
        self.assertEqual(renumerar.renumber('D07', {7: 8})[0], 'D08')

    def test_crlf_is_preserved(self):
        self.assertEqual(renumerar.renumber('D92\r\nD93\r\n', {92: 1, 93: 2})[0], 'D01\r\nD02\r\n')

    def test_occupied_target_is_refused(self):
        decisions = '- **D100 A.** x\n- **D92 B.** y\n'
        with self.assertRaisesRegex(ValueError, 'D100'):
            renumerar.check_targets({92: 100}, decisions)
        # Si el ocupante también se mueve, no hay choque.
        renumerar.check_targets({92: 100, 100: 114}, decisions)

    def test_bad_arguments(self):
        for bad in (['D98-D92=D1'], ['92=106'], ['D1=D5', 'D2=D5'], ['D1=D5', 'D1=D6']):
            with self.assertRaises(ValueError):
                renumerar.parse_mapping(bad)


if __name__ == '__main__':
    unittest.main()
