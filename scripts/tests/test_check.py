import importlib.util
import re
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
        if (check.ROOT / 'evidence').is_dir():
            shutil.copytree(check.ROOT / 'evidence', self.root / 'evidence')
        # Destinos enlazados desde los documentos copiados que existen en el repositorio: se
        # sustituyen por un archivo mínimo, así la prueba no depende de una lista a mano.
        for name in check.REQUIRED:
            for target in check.LINK.findall((self.root / name).read_text(encoding='utf-8')):
                if re.match(r'^[a-z]+:', target) or target.startswith('#'):
                    continue
                rel = (Path(name).parent / target.split('#', 1)[0])
                real = (check.ROOT / rel).resolve()
                copy = (self.root / rel).resolve()
                if real.is_file() and not copy.exists():
                    copy.parent.mkdir(parents=True, exist_ok=True)
                    copy.write_text('x', encoding='utf-8')
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
        self.edit_matrix('| R03 |', '| R03 | T03z: caso adicional. | Pendiente |\n| R03 |')
        self.assertEqual(check.check(self.root), [])

    def test_requirement_without_row_fails(self):
        text = self.matrix.read_text(encoding='utf-8')
        lines = [line for line in text.splitlines() if not line.startswith('| R05 |')]
        self.matrix.write_text('\n'.join(lines) + '\n', encoding='utf-8')
        self.assertIn('R05 no tiene ningún caso de verificación en la matriz.', check.check(self.root))

    def test_duplicate_test_id_fails(self):
        self.edit_matrix('| R02 | T02:', '| R02 | T01:')
        self.assertIn('Los identificadores de prueba deben ser únicos.', check.check(self.root))

    def add_pending_row(self):
        # Fila propia de la prueba, para no depender del estado real de la matriz.
        self.edit_matrix('| R03 |', '| R03 | T99: caso de la prueba. | Pendiente |\n| R03 |')

    def test_fulfilled_status_requires_existing_evidence(self):
        self.add_pending_row()
        self.edit_matrix('caso de la prueba. | Pendiente |', 'caso de la prueba. | Cumplido |')
        self.assertTrue(any('requiere un enlace' in e for e in check.check(self.root)))
        self.edit_matrix('| Cumplido |', '| Cumplido — [evidencia](../../evidence/t01.md) |')
        self.assertTrue(any('enlace roto' in e for e in check.check(self.root)))
        (self.root / 'evidence').mkdir(exist_ok=True)
        (self.root / 'evidence/t01.md').write_text('ok', encoding='utf-8')
        self.assertEqual(check.check(self.root), [])

    def test_unknown_status_and_missing_file_fail(self):
        self.add_pending_row()
        self.edit_matrix('caso de la prueba. | Pendiente |', 'caso de la prueba. | Hecho |')
        (self.root / 'specs/v1/decisiones.md').unlink()
        errors = check.check(self.root)
        self.assertTrue(any("estado desconocido 'Hecho'" in e for e in errors))
        self.assertIn('Falta o está vacío: specs/v1/decisiones.md', errors)

    def test_requirement_numbering_gap_fails(self):
        req = self.root / 'specs/v1/requisitos.md'
        req.write_text(req.read_text(encoding='utf-8').replace('- **R12**', '- **R13**'), encoding='utf-8')
        self.assertTrue(any('sin huecos' in e for e in check.check(self.root)))

    def append(self, name, extra):
        path = self.root / name
        path.write_text(path.read_text(encoding='utf-8') + extra, encoding='utf-8')

    def test_duplicate_decision_fails(self):
        self.append('specs/v1/decisiones.md', '\n- **D100 Copia.** Repetida.\n')
        self.assertIn('La decisión D100 está repetida.', check.check(self.root))

    def test_decision_outside_blocks_fails(self):
        self.append('specs/v1/decisiones.md', '\n- **D900 Suelta.** Sin bloque.\n')
        self.assertTrue(any('D900 no está en ningún bloque' in e for e in check.check(self.root)))

    def test_dangling_reference_fails_unless_reserved(self):
        self.append('PLAN.md', '\nVer D900.\n')
        self.assertIn('PLAN.md: cita la decisión inexistente D900.', check.check(self.root))
        self.append('PLAN.md', '\nAntes `D901`.\n')
        self.assertNotIn('PLAN.md: cita la decisión inexistente D901.', check.check(self.root))
        self.append('specs/v1/numeracion.md', '| D900–D909 | `rama-de-prueba` | Reservado |\n')
        self.assertEqual(check.check(self.root), [])

    def test_overlapping_blocks_fail(self):
        self.append('specs/v1/numeracion.md', '| D100–D200 | `otra` | Reservado |\n')
        self.assertTrue(any('se solapan' in e for e in check.check(self.root)))


if __name__ == '__main__':
    unittest.main()
