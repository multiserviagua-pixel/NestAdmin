import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

MODULE = Path(__file__).resolve().parents[1] / 'evaluate.py'
spec = importlib.util.spec_from_file_location('evaluate', MODULE)
evaluate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evaluate)


class EvaluationTests(unittest.TestCase):
    def setUp(self):
        self.cases = evaluate.load_cases()

    def perfect_predictions(self):
        predictions = {}
        for case in self.cases:
            exp = case['expected']
            if case['kind'] == 'invoice':
                result = {'fields': exp['fields'].copy(), 'issues': exp['issues'].copy()}
            else:
                result = {'category': exp['category'], 'draft': exp['draft_rubric']['must_include_any'][0]}
            predictions[case['id']] = {'id': case['id'], 'result': result}
        return predictions

    def test_corpus_and_perfect_predictions(self):
        self.assertEqual(sum(c['kind'] == 'invoice' for c in self.cases), 10)
        self.assertEqual(sum(c['kind'] == 'email' for c in self.cases), 20)
        metrics = evaluate.score(self.cases, self.perfect_predictions())
        self.assertEqual(metrics['extraction']['accuracy'], 1)
        self.assertEqual(metrics['issues']['recall'], 1)
        self.assertEqual(metrics['classification']['accuracy'], 1)
        self.assertEqual(metrics['draft_heuristic']['rate'], 1)
        self.assertIsNone(metrics['performance']['latency_ms_mean'])

    def test_missing_error_hallucination_and_bad_draft(self):
        predictions = self.perfect_predictions()
        del predictions['D01']
        predictions['D02'] = {'id': 'D02', 'error': 'model unavailable'}
        predictions['D07']['result']['fields']['tax'] = '21.00'
        predictions['E01']['result']['draft'] = 'Factura enviada; pago confirmado'
        metrics = evaluate.score(self.cases, predictions)
        self.assertEqual(metrics['cases']['missing'], 1)
        self.assertEqual(metrics['cases']['errors'], 1)
        self.assertEqual(metrics['extraction']['hallucinated_absent_fields'], 1)
        self.assertEqual(metrics['draft_heuristic']['passed'], 19)
        self.assertEqual(metrics['extraction']['correct_fields'], 47)

    def test_missing_nullable_field_is_not_counted_correct(self):
        predictions = self.perfect_predictions()
        del predictions['D07']['result']['fields']['tax']
        metrics = evaluate.score(self.cases, predictions)
        self.assertEqual(metrics['extraction']['correct_fields'], 59)
        self.assertEqual(metrics['extraction']['hallucinated_absent_fields'], 0)

    def test_performance_coverage_and_error_issue_recall(self):
        predictions = self.perfect_predictions()
        predictions['D08'] = {'id': 'D08', 'error': 'timeout', 'latency_ms': 42, 'peak_memory_bytes': 1234}
        metrics = evaluate.score(self.cases, predictions)
        self.assertEqual(metrics['performance']['latency_ms_mean'], 42)
        self.assertEqual(metrics['performance']['memory_samples'], 1)
        self.assertEqual(metrics['issues']['false_negative'], 1)

    def test_prediction_validation_rejects_duplicates_unknown_and_invalid_timings(self):
        invalid_sets = [
            [{'id': 'D01', 'result': {}}, {'id': 'D01', 'result': {}}],
            [{'id': 'unknown', 'result': {}}],
            [{'id': 'D01', 'result': {}, 'latency_ms': -1}],
            [{'id': 'D01', 'result': {}, 'error': 'failure'}],
        ]
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'predictions.jsonl'
            for items in invalid_sets:
                path.write_text(''.join(json.dumps(item) + '\n' for item in items), encoding='utf-8')
                with self.subTest(items=items), self.assertRaises(ValueError):
                    evaluate.load_predictions(path, self.cases)


if __name__ == '__main__':
    unittest.main()
