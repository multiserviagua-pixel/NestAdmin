import importlib.util
from pathlib import Path
import shutil
import tempfile
import unittest

MODULE = Path(__file__).resolve().parents[1] / 'check.py'
spec = importlib.util.spec_from_file_location('check', MODULE)
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


class CheckTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        for name in check.REQUIRED:
            target = self.root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(check.ROOT / name, target)
        # Destinos enlazados desde los documentos copiados.
        for name in ('evaluation/README.md', '.github/workflows/verify.yml'):
            target = self.root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text('x', encoding='utf-8')
        self.matrix = self.root / 'specs/v1/trazabilidad.md'

    def tearDown(self):
        self.tmp.cleanup()

    def edit_matrix(self, old, new):
        text = self.matrix.read_text(encoding='utf-8')
        self.assertIn(old, text)
        self.matrix.write_text(text.replace(old, new, 1), encoding='utf-8')

    def test_repository_passes(self):
        self.assertEqual(check.check(check.ROOT), [])
        self.assertEqual(check.check(self.root), [])

    def test_extra_test_row_for_a_requirement_is_allowed(self):
        self.edit_matrix('| R03 |', '| R03 | T03b: caso adicional. | Pendiente |\n| R03 |')
        self.assertEqual(check.check(self.root), [])

    def test_requirement_without_row_fails(self):
        text = self.matrix.read_text(encoding='utf-8')
        lines = [line for line in text.splitlines() if not line.startswith('| R05 |')]
        self.matrix.write_text('\n'.join(lines) + '\n', encoding='utf-8')
        self.assertIn('R05 no tiene ningún caso de verificación en la matriz.', check.check(self.root))

    def test_duplicate_test_id_fails(self):
        self.edit_matrix('| R02 | T02:', '| R02 | T01:')
        self.assertIn('Los identificadores de prueba deben ser únicos.', check.check(self.root))

    def test_fulfilled_status_requires_existing_evidence(self):
        self.edit_matrix('comprobar que el proceso termina. | Pendiente |', 'comprobar que el proceso termina. | Cumplido |')
        self.assertTrue(any('requiere un enlace' in e for e in check.check(self.root)))
        self.edit_matrix('| Cumplido |', '| Cumplido — [evidencia](../../evidence/t01.md) |')
        self.assertTrue(any('enlace roto' in e for e in check.check(self.root)))
        (self.root / 'evidence').mkdir()
        (self.root / 'evidence/t01.md').write_text('ok', encoding='utf-8')
        self.assertEqual(check.check(self.root), [])

    def test_unknown_status_and_missing_file_fail(self):
        self.edit_matrix('termina. | Pendiente |', 'termina. | Hecho |')
        (self.root / 'specs/v1/decisiones.md').unlink()
        errors = check.check(self.root)
        self.assertTrue(any("estado desconocido 'Hecho'" in e for e in errors))
        self.assertIn('Falta o está vacío: specs/v1/decisiones.md', errors)

    def test_requirement_numbering_gap_fails(self):
        req = self.root / 'specs/v1/requisitos.md'
        req.write_text(req.read_text(encoding='utf-8').replace('- **R12**', '- **R13**'), encoding='utf-8')
        self.assertTrue(any('sin huecos' in e for e in check.check(self.root)))


if __name__ == '__main__':
    unittest.main()
