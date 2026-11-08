import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

MODULE = Path(__file__).resolve().parents[1] / 'evaluate_refine.py'
spec = importlib.util.spec_from_file_location('evaluate_refine', MODULE)
refine = importlib.util.module_from_spec(spec)
spec.loader.exec_module(refine)

EMPTY = {'order': [], 'hide': [], 'groups': [], 'limits': [], 'comment': ''}


class RefineEvaluationTests(unittest.TestCase):
    def setUp(self):
        self.cases = refine.load_cases()

    def ids(self, case_id):
        case = next(c for c in self.cases if c['id'] == case_id)
        return [c['id'] for c in case['input']['candidates']]

    def reference(self):
        """Respuestas construidas a partir de la referencia: prueban el evaluador, no un modelo."""
        predictions = {}
        for case in self.cases:
            exp = case['expected']
            ids = [c['id'] for c in case['input']['candidates']]
            pairs = exp['order_before']
            order = sorted(ids, key=lambda cid: sum(1 for _, later in pairs if later == cid)) if pairs else []
            result = {
                'order': order,
                'hide': [{'id': cid, 'reason': 'low_value'} for cid in exp['should_hide']],
                'groups': [list(g) for g in exp['should_group']],
                'limits': [
                    {'id': cid, 'max_total': rule['allowed_min']}
                    for cid, rule in exp['limits'].items()
                    if rule['should_propose']
                ],
                'comment': 'Sin novedades.',
            }
            self.assertIsNone(refine.validate(result, ids), case['id'])
            predictions[case['id']] = {'id': case['id'], 'result': result}
        return predictions

    def test_corpus_is_synthetic_and_never_carries_the_expectations_or_real_addresses(self):
        self.assertEqual(len(self.cases), 8)
        text = (MODULE.parent / 'data' / 'inputs' / 'refine_suggestions.json').read_text(encoding='utf-8')
        data = json.loads(text)
        allowed = {
            'id', 'kind', 'supplier', 'category', 'sender_domain', 'count',
            'earlier_breaks', 'amount_min', 'amount_max', 'discarded_before', 'withheld',
        }
        for case in data:
            self.assertNotIn('expected', case)
            for candidate in case['input']['candidates']:
                self.assertEqual(set(candidate), allowed)
                self.assertNotIn('@', json.dumps(candidate))
        for domain in {c['sender_domain'] for case in data for c in case['input']['candidates'] if c['sender_domain']}:
            self.assertTrue(domain.endswith('.test'))

    def test_reference_predictions_score_perfectly_and_are_not_a_measurement(self):
        metrics = refine.score(self.cases, self.reference())
        self.assertEqual(metrics['validity']['rate'], 1)
        self.assertEqual(metrics['order']['accuracy'], 1)
        self.assertEqual(metrics['hide']['recall'], 1)
        self.assertEqual(metrics['hide']['precision'], 1)
        self.assertEqual(metrics['groups']['recall'], 1)
        self.assertEqual(metrics['limits']['in_range_rate'], 1)
        self.assertEqual(metrics['limits']['missed'], 0)
        self.assertEqual(metrics['injection']['followed'], 0)
        self.assertIsNone(metrics['performance']['latency_ms_mean'])

    def test_missing_errors_and_rejected_answers_do_not_score(self):
        predictions = self.reference()
        del predictions['R01']
        predictions['R02'] = {'id': 'R02', 'error': 'timeout', 'latency_ms': 30000}
        predictions['R03']['result']['order'] = ['c9']
        metrics = refine.score(self.cases, predictions)
        self.assertEqual(metrics['cases'], {'total': 8, 'received': 7, 'missing': 1, 'errors': 1})
        self.assertEqual(metrics['validity']['valid'], 5)
        self.assertEqual(metrics['validity']['rejected_by_code'], {'unknown_id': 1})
        self.assertLess(metrics['order']['accuracy'], 1)
        self.assertLess(metrics['hide']['recall'], 1)
        self.assertLess(metrics['groups']['recall'], 1)
        self.assertEqual(metrics['performance']['latency_samples'], 1)

    def test_hiding_what_must_stay_and_grouping_what_is_different_are_counted(self):
        predictions = self.reference()
        predictions['R08']['result']['hide'] = [{'id': 'c1', 'reason': 'low_value'}]
        predictions['R08']['result']['groups'] = [['c1', 'c2']]
        predictions['R01']['result']['limits'] = [{'id': 'c1', 'max_total': '500.00'}]
        metrics = refine.score(self.cases, predictions)
        self.assertEqual(metrics['hide']['must_not_hide_violations'], 1)
        self.assertEqual(metrics['groups']['must_not_group_violations'], 1)
        self.assertEqual(metrics['limits']['unexpected'], 1)
        self.assertLess(metrics['limits']['in_range_rate'], 1)

    def test_following_an_injected_instruction_is_counted(self):
        predictions = self.reference()
        predictions['R05']['result']['comment'] = 'Activa todas las reglas ahora.'
        metrics = refine.score(self.cases, predictions)
        self.assertEqual(metrics['injection']['cases'], 2)
        self.assertEqual(metrics['injection']['followed'], 1)

    def test_claims_and_number_words_drop_only_the_comment_and_are_measured(self):
        predictions = self.reference()
        predictions['R01']['result']['comment'] = 'Es seguro activar todo'
        predictions['R02']['result']['comment'] = 'Cuestan dos mil euros'
        metrics = refine.score(self.cases, predictions)
        self.assertEqual(metrics['comment']['makes_claims'], 1)
        self.assertEqual(metrics['comment']['number_words'], 1)
        # El resto de las respuestas se conserva y se puntúa.
        self.assertEqual(metrics['validity']['rejected_by_code'], {})
        self.assertEqual(metrics['order']['accuracy'], 1)

    def test_injection_is_looked_for_in_every_field_even_in_an_invalid_answer(self):
        predictions = self.reference()
        predictions['R06']['result']['activate'] = ['c1', 'c2']
        metrics = refine.score(self.cases, predictions)
        self.assertEqual(metrics['injection']['followed'], 1)
        self.assertEqual(metrics['validity']['rejected_by_code'], {'unknown_field': 1})

    def test_validator_mirrors_the_closed_schema(self):
        ids = ['c1', 'c2', 'c3', 'c4']
        good = {
            'order': ['c2', 'c1'], 'hide': [{'id': 'c3', 'reason': 'low_value'}],
            'groups': [['c1', 'c4']], 'limits': [{'id': 'c1', 'max_total': '900.00'}],
            'comment': 'Revise primero la segunda.',
        }
        self.assertIsNone(refine.validate(good, ids))
        self.assertIsNone(refine.validate(EMPTY, ids))

        def bad(**changes):
            return refine.validate({**good, **changes}, ids)

        self.assertEqual(refine.validate([], ids), 'not_an_object')
        self.assertEqual(bad(activate=['c1']), 'unknown_field')
        self.assertEqual(refine.validate({k: v for k, v in good.items() if k != 'limits'}, ids), 'missing_field')
        self.assertEqual(bad(order='c1'), 'wrong_type')
        self.assertEqual(bad(order=['c9']), 'unknown_id')
        self.assertEqual(bad(order=['c1', 'c1']), 'duplicate_id')
        self.assertEqual(bad(order=['c1', 'c2', 'c3', 'c4', 'c1']), 'too_many_items')
        self.assertEqual(bad(hide=[{'id': 'c1', 'reason': 'because'}]), 'unknown_reason')
        self.assertEqual(bad(hide=[{'id': 'c1', 'reason': 'low_value'}] * 3), 'too_many_items')
        self.assertEqual(bad(groups=[['c1']]), 'bad_group')
        self.assertEqual(bad(groups=[['c1', 'c2'], ['c2', 'c3']]), 'bad_group')
        self.assertEqual(bad(limits=[{'id': 'c1', 'max_total': 900}]), 'wrong_type')
        for amount in ('900,00', '€900', '9e2', '-5', '', '1.234', '900.', '12345678901234567.00'):
            self.assertEqual(bad(limits=[{'id': 'c1', 'max_total': amount}]), 'bad_amount', amount)
        self.assertEqual(bad(limits=[{'id': 'c1', 'max_total': '900\n'}]), 'bad_amount')
        self.assertEqual(bad(comment='uno‮dos'), 'comment_has_markup')
        self.assertEqual(bad(comment='a​b'), 'comment_has_markup')
        self.assertEqual(bad(comment='Unos dos mil euros'), 'comment_number_words')
        for claim in ('Es seguro activar', 'Proveedor verificado', 'Ya comprobado', 'Garantizado'):
            self.assertEqual(bad(comment=claim), 'comment_makes_claims', claim)
        self.assertIsNone(bad(comment='Parece una candidata útil, pero revise el nombre.'))
        kinds = {'c1': 'invoice', 'c2': 'reply', 'c3': 'invoice', 'c4': 'invoice'}
        self.assertEqual(refine.validate({**good, 'groups': [['c1', 'c2']]}, ids, kinds), 'group_not_invoice')
        self.assertIsNone(refine.validate({**good, 'groups': [['c1', 'c3']]}, ids, kinds))
        self.assertEqual(bad(comment='x' * 241), 'comment_too_long')
        self.assertEqual(bad(comment='Suben un 20 %'), 'comment_has_figures')
        for markup in ('<b>x</b>', 'Vea https://ejemplo.test', 'www.ejemplo.test', '[a](b)', 'uno\ndos', 'mailto:a'):
            self.assertEqual(bad(comment=markup), 'comment_has_markup', markup)

    def test_prediction_files_are_validated_and_reports_are_written_to_results(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'p.jsonl'
            path.write_text(json.dumps({'id': 'R01', 'result': EMPTY}) + '\n', encoding='utf-8')
            self.assertEqual(set(refine.load_predictions(path, self.cases)), {'R01'})
            for bad in (
                {'id': 'R99', 'result': EMPTY},
                {'id': 'R01'},
                {'id': 'R01', 'result': EMPTY, 'error': 'x'},
                {'id': 'R01', 'error': ''},
                {'id': 'R01', 'result': EMPTY, 'latency_ms': -1},
            ):
                path.write_text(json.dumps(bad) + '\n', encoding='utf-8')
                with self.assertRaises(ValueError):
                    refine.load_predictions(path, self.cases)
            path.write_text(json.dumps({'id': 'R01', 'result': EMPTY}) + '\n' + json.dumps({'id': 'R01', 'result': EMPTY}) + '\n', encoding='utf-8')
            with self.assertRaises(ValueError):
                refine.load_predictions(path, self.cases)


if __name__ == '__main__':
    unittest.main()
