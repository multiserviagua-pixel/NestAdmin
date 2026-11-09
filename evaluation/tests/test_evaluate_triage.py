import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

MODULE = Path(__file__).resolve().parents[1] / 'evaluate_triage.py'
spec = importlib.util.spec_from_file_location('evaluate_triage', MODULE)
triage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(triage)


def ok(category, needs_reply, draft=None, **extra):
    result = {'category': category, 'needs_reply': needs_reply, **extra}
    if draft is not None:
        result['draft'] = draft
    return {'result': result}


class LocalLayerTests(unittest.TestCase):
    def setUp(self):
        self.cases = triage.load_cases()

    def test_corpus_size_and_shape(self):
        self.assertGreaterEqual(len(self.cases), 140)
        for case in self.cases:
            self.assertEqual(set(case['input']), {'from', 'to', 'date', 'subject', 'body', 'headers'})
            for header in case['input']['headers']:
                self.assertEqual(set(header), {'name', 'value'})
            self.assertIn(case['expected']['route'], triage.ROUTES)
            self.assertIn(case['expected']['category'], triage.CATEGORIES)
            self.assertEqual('draft_rubric' in case['expected'], case['expected']['needs_reply'])

    def test_corpus_is_synthetic(self):
        text = (MODULE.parent / 'data' / 'inputs' / 'triage.json').read_text(encoding='utf-8')
        for case in json.loads(text):
            sender = case['input']['from']
            domain = sender.rsplit('@', 1)[1].rstrip('>')
            self.assertTrue(domain.endswith(('.test', '.example')), sender)

    def test_python_route_matches_the_hand_labelled_corpus(self):
        # Parity: the Rust implementation passes the same labels in
        # crates/core/tests/triage_corpus.rs.
        for case in self.cases:
            got = triage.local_route(case['input']['from'], triage.case_headers(case))
            self.assertEqual(got, case['expected']['route'], case['id'])

    def test_local_layer_reports_zero_hidden_fn_on_the_corpus(self):
        local = triage.score_local(self.cases)
        self.assertEqual(local['hidden_fn'], 0)
        self.assertEqual(local['route_correct'], local['total'])

    def test_hidden_fn_counts_a_reply_needed_mail_sent_to_informativos(self):
        case = {'id': 'X', 'input': {'from': 'No-Reply <noreply@a.test>', 'headers': []},
                'expected': {'route': 'informativo', 'needs_reply': True}}
        local = triage.score_local([case])
        self.assertEqual((local['hidden_fn'], local['hidden_fn_ids']), (1, ['X']))
        self.assertFalse(triage.gates(local, None)['hidden_fn_zero'])

    def test_signals_mirror_mail_signals(self):
        s = triage.signals
        for sender in ('noreply@a.test', 'No-Reply@a.test', 'no.reply@a.test', 'donotreply@a.test',
                       'noreply+x@a.test', 'Servicio <no-reply@a.test>', '"S.A., X" <NOREPLY@a.test>',
                       'account-security-noreply@a.test', 'ventas.noreply.es@a.test'):
            self.assertEqual(s(sender, None), ['noreply_address'], sender)
        for sender in ('arnold@a.test', 'noreplyinc@e.test', 'honoreply@a.test', 'ana@noreply.test',
                       'sin-arroba', '', '@a.test', None):
            self.assertEqual(s(sender, None), [], sender)
        self.assertEqual(s('Mail <MAILER-DAEMON@a.test>', None), ['bounce_address'])
        self.assertEqual(s('x@a.test', [('Auto-Submitted', 'auto-generated (x)')]), ['auto_submitted'])
        self.assertEqual(s('x@a.test', [('Auto-Submitted', 'auto-generated; k=v')]), ['auto_submitted'])
        for value in ('no', ' No ', 'NO (humano)', 'no; x=y', ''):
            self.assertEqual(s('x@a.test', [('Auto-Submitted', value)]), [], value)
        self.assertEqual(s('x@a.test', [('Precedence', ' BULK ')]), ['precedence_bulk'])
        self.assertEqual(s('x@a.test', [('precedence', 'junk')]), ['precedence_junk'])
        self.assertEqual(s('x@a.test', [('Precedence', 'first-class')]), [])
        self.assertEqual(s('x@a.test', [('List-Id', '  ')]), [])
        self.assertEqual(s('x@a.test', [('Return-Path', '  < >  ')]), ['null_return_path'])
        self.assertEqual(s('x@a.test', [('Return-Path', '<x@a.test>')]), [])

    def test_route_precedence(self):
        r = triage.local_route
        self.assertEqual(r('mailer-daemon@a.test', [('Auto-Submitted', 'auto-replied')]), 'rebote')
        self.assertEqual(r('ana@a.test', [('List-Unsubscribe', '<mailto:x@a.test>'), ('List-Id', '<x>'), ('Precedence', 'bulk')]), 'ia')
        self.assertEqual(r('ana@a.test', [('Precedence', 'junk')]), 'informativo')


class AiScoringTests(unittest.TestCase):
    """Hand-built predictions: they test the evaluator, not a model."""

    def setUp(self):
        self.cases = triage.load_cases()

    def reference(self):
        out = {}
        for case in self.cases:
            exp = case['expected']
            if exp['needs_reply']:
                word = exp['draft_rubric']['must_include_any'][0]
                out[case['id']] = {'id': case['id'], **ok(exp['category'], True, f'Hola, sobre su {word}: lo revisamos.')}
            else:
                out[case['id']] = {'id': case['id'], **ok(exp['category'], False)}
        return out

    def test_perfect_predictions(self):
        report = triage.score(self.cases, self.reference())
        ai = report['ai']
        self.assertEqual(ai['degraded']['total'], 0)
        self.assertEqual(ai['needs_reply']['true']['recall'], 1.0)
        self.assertEqual(ai['needs_reply']['true']['precision'], 1.0)
        self.assertEqual(ai['needs_reply']['false']['recall'], 1.0)
        self.assertEqual(ai['draft_rubric']['rate'], 1.0)
        for c in triage.CATEGORIES:
            self.assertEqual(ai['category']['per_category'][c]['recall'], 1.0)
        self.assertEqual(report['gates'], {'hidden_fn_zero': True, 'needs_reply_true_recall_at_least_0_95': True})

    def test_missing_and_errors_count_as_failures_and_stay_in_the_denominator(self):
        predictions = self.reference()
        needs = [c['id'] for c in self.cases if c['expected']['needs_reply']]
        n_true = len(needs)
        del predictions[needs[0]]
        predictions[needs[1]] = {'id': needs[1], 'error': 'timeout'}
        ai = triage.score(self.cases, predictions)['ai']
        self.assertEqual((ai['cases']['missing'], ai['cases']['errors']), (1, 1))
        self.assertEqual(ai['needs_reply']['true']['false_negative'], 2)
        self.assertEqual(ai['needs_reply']['true']['recall'], (n_true - 2) / n_true)
        self.assertEqual(ai['needs_reply']['confusion']['expected_true']['missing_or_error'], 2)
        self.assertEqual(ai['draft_rubric']['passed'], n_true - 2)
        self.assertEqual(ai['degraded']['total'], 2)

    def test_false_verdict_on_a_reply_needed_mail_hurts_recall_and_gate(self):
        predictions = self.reference()
        needs = [c for c in self.cases if c['expected']['needs_reply']]
        for case in needs[:10]:
            predictions[case['id']] = {'id': case['id'], **ok(case['expected']['category'], False)}
        report = triage.score(self.cases, predictions)
        true = report['ai']['needs_reply']['true']
        self.assertEqual(true['false_negative'], 10)
        self.assertEqual(report['ai']['needs_reply']['false']['false_positive'], 10)
        self.assertLess(true['recall'], 0.95)
        self.assertFalse(report['gates']['needs_reply_true_recall_at_least_0_95'])
        self.assertEqual(report['ai']['draft_rubric']['passed'], len(needs) - 10)

    def test_wrong_category_and_confusion_matrix(self):
        predictions = self.reference()
        case = next(c for c in self.cases if c['expected']['needs_reply'] and c['expected']['category'] == 'solicitud_factura')
        predictions[case['id']] = {'id': case['id'], **ok('consulta_pago', True, 'Su factura y pago.')}
        ai = triage.score(self.cases, predictions)['ai']
        self.assertEqual(ai['category']['confusion']['solicitud_factura']['consulta_pago'], 1)
        self.assertEqual(ai['category']['per_category']['solicitud_factura']['false_negative'], 1)
        self.assertEqual(ai['category']['per_category']['consulta_pago']['false_positive'], 1)

    def test_draft_rubric_and_draft_error_and_degraded(self):
        predictions = self.reference()
        needs = [c for c in self.cases if c['expected']['needs_reply']]
        a, b, c = needs[:3]
        predictions[a['id']] = {'id': a['id'], **ok(a['expected']['category'], True, 'Pago confirmado. Factura enviada.')}
        predictions[b['id']] = {'id': b['id'], **ok(b['expected']['category'], True, draft_error='invalid_response')}
        predictions[c['id']] = {'id': c['id'], **ok(c['expected']['category'], True, '')}
        ai = triage.score(self.cases, predictions)['ai']
        self.assertEqual(ai['draft_rubric']['passed'], len(needs) - 3)
        self.assertEqual(ai['degraded']['draft_errors'], 1)

    def test_loader_validates_the_contract(self):
        def load(lines):
            with tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp) / 'p.jsonl'
                path.write_text('\n'.join(json.dumps(x) for x in lines), encoding='utf-8')
                return triage.load_predictions(path, self.cases)

        first = self.cases[0]['id']
        self.assertEqual(len(load([{'id': first, **ok('otros', False)}])), 1)
        bad = (
            [{'id': 'ZZ', **ok('otros', False)}],
            [{'id': first, **ok('otros', False)}, {'id': first, **ok('otros', False)}],
            [{'id': first, 'result': {'category': 'otros', 'needs_reply': 'no'}}],
            [{'id': first, 'result': {'category': 'otros', 'needs_reply': False}, 'error': 'x'}],
            [{'id': first}],
            [{'id': first, 'error': ' '}],
            [{'id': first, **ok('otros', False), 'latency_ms': -1}],
        )
        for lines in bad:
            with self.assertRaises(ValueError):
                load(lines)

    def test_local_only_has_no_ai_section(self):
        report = triage.score(self.cases)
        self.assertIsNone(report['ai'])
        self.assertIsNone(report['gates']['needs_reply_true_recall_at_least_0_95'])


if __name__ == '__main__':
    unittest.main()
