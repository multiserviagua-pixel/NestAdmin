import copy
import importlib.util
import json
import re
import tempfile
import unittest
from pathlib import Path

MODULE = Path(__file__).resolve().parents[1] / 'evaluate_learn.py'
spec = importlib.util.spec_from_file_location('evaluate_learn', MODULE)
learn = importlib.util.module_from_spec(spec)
spec.loader.exec_module(learn)


def passing(leaks=None):
    return {'result': {'outcome': 'pass', 'reason': None, 'leaks': leaks or []}}


def discarded(code):
    return {'result': {'outcome': 'discard', 'reason': code, 'leaks': []}}


class LearnEvaluationTests(unittest.TestCase):
    """Hand-built predictions: they test the evaluator, not the learning code."""

    def setUp(self):
        self.cases = learn.load_cases()

    def perfect(self):
        """What a flawless local layer would answer: discard exactly what must be discarded."""
        out = {}
        for case in self.cases:
            code = case['expected']['discard']
            out[case['id']] = {'id': case['id'], **(passing() if code is None else discarded(code))}
        return out

    def perfect_ai(self):
        out = {}
        for case in self.cases:
            exp = case['expected']
            if exp['discard'] is None:
                out[case['id']] = {'id': case['id'], 'result': {
                    'category': exp['category'], 'needs_reply': exp['needs_reply'], 'reply_kind': exp['reply_kind']}}
        return out

    def case(self, group):
        return next(c for c in self.cases if c['expected']['group'] == group)

    # ---- corpus

    def test_corpus_shape(self):
        self.assertGreaterEqual(len(self.cases), 40)
        groups = {c['expected']['group'] for c in self.cases}
        for wanted in ('cita_es', 'cita_en', 'cita_ca', 'cita_fr', 'cita_it', 'cita_de', 'cita_pt',
                       'outlook_aplanado', 'firma_suelta', 'datos_sensibles', 'nombre_remitente',
                       'reenvio_interno', 'inyeccion_en_cita', 'automatico', 'normal'):
            self.assertIn(wanted, groups)
        for case in self.cases:
            exp = case['expected']
            self.assertEqual(set(case['input']),
                             {'from', 'to', 'date', 'subject', 'body', 'headers', 'thread_key', 'own_reply'})
            self.assertTrue(exp['discard'] is None or exp['discard'] in learn.DISCARD_CODES, case['id'])
            for item in exp['forbidden']:
                self.assertIn(item['kind'], learn.LEAK_KINDS, case['id'])
                self.assertTrue(item['text'].strip(), case['id'])
            if exp['discard'] is None:
                self.assertIn(exp['category'], learn.CATEGORIES, case['id'])
                self.assertIn(exp['reply_kind'], learn.REPLY_KINDS, case['id'])
                self.assertIsInstance(exp['needs_reply'], bool, case['id'])
                self.assertTrue(exp['forbidden'], case['id'])
            else:
                self.assertNotIn('category', exp, case['id'])

    def test_corpus_covers_every_discard_code_that_the_corpus_can_express(self):
        codes = {c['expected']['discard'] for c in self.cases}
        for wanted in ('automatic', 'unchecked', 'no_sender', 'no_thread', 'no_own_reply', 'quote_dropped',
                       'not_reply_to_sender', 'signature_dropped', 'empty_text'):
            self.assertIn(wanted, codes)
        self.assertIn(None, codes)

    def test_corpus_is_synthetic(self):
        text = (MODULE.parent / 'data' / 'inputs' / learn.NAME).read_text(encoding='utf-8')
        addresses = re.findall(r'[\w.+-]+@([\w-]+(?:\.[\w-]+)+)', text)
        self.assertTrue(addresses)
        for domain in addresses:
            self.assertTrue(domain.endswith(('.test', '.example')), domain)
        gold = (MODULE.parent / 'data' / 'expected' / learn.NAME).read_text(encoding='utf-8')
        for domain in re.findall(r'[\w.+-]+@([\w-]+(?:\.[\w-]+)+)', gold):
            self.assertTrue(domain.endswith(('.test', '.example')), domain)

    def test_sensitive_data_cases_exist_with_the_promised_kinds(self):
        kinds = {item['kind'] for c in self.cases for item in c['expected']['forbidden']}
        for wanted in ('name', 'alias_local', 'signature', 'iban', 'dni', 'phone', 'card', 'secret', 'email',
                       'url', 'quote', 'injection'):
            self.assertIn(wanted, kinds)

    def test_a_reply_to_the_sender_and_an_internal_forward_differ_only_by_recipients(self):
        internal = [c for c in self.cases if c['expected']['group'] == 'reenvio_interno']
        self.assertEqual(len(internal), 2)
        for case in internal:
            sender = case['input']['from']
            addresses = re.findall(r'[\w.+-]+@[\w.-]+', ' '.join(case['input']['own_reply']['to']))
            self.assertNotIn(re.findall(r'[\w.+-]+@[\w.-]+', sender)[0], addresses)

    # ---- local layer: gates

    def test_a_flawless_local_layer_meets_both_gates(self):
        report = learn.score(self.cases, self.perfect())
        self.assertEqual(report['leaks']['cases_with_leaks'], 0)
        self.assertEqual(report['discards']['correct'], report['discards']['expected'])
        self.assertEqual(report['false_discards']['count'], 0)
        self.assertTrue(report['gates']['zero_leaks'])
        self.assertTrue(report['gates']['expected_discards_hit'])

    def test_one_leak_fails_the_leak_gate_and_names_case_and_kind(self):
        predictions = self.perfect()
        case = self.case('datos_sensibles')
        index = next(i for i, f in enumerate(case['expected']['forbidden']) if f['kind'] == 'iban')
        predictions[case['id']] = {'id': case['id'], **passing([{'index': index, 'kind': 'iban'}])}
        report = learn.score(self.cases, predictions)
        self.assertFalse(report['gates']['zero_leaks'])
        self.assertTrue(report['gates']['expected_discards_hit'])
        self.assertEqual(report['leaks']['cases'], [{'id': case['id'], 'kinds': ['iban'], 'count': 1}])
        self.assertEqual(report['leaks']['by_kind']['iban'], 1)
        self.assertEqual(report['leaks']['by_kind']['dni'], 0)

    def test_the_kind_comes_from_the_corpus_not_from_the_prediction(self):
        predictions = self.perfect()
        case = self.case('firma_suelta')
        index = next(i for i, f in enumerate(case['expected']['forbidden']) if f['kind'] == 'signature')
        predictions[case['id']] = {'id': case['id'], **passing([{'index': index, 'kind': 'whatever'}])}
        report = learn.score(self.cases, predictions)
        self.assertEqual(report['leaks']['cases'][0]['kinds'], ['signature'])

    def test_a_leak_index_out_of_range_is_an_error(self):
        predictions = self.perfect()
        case = self.case('datos_sensibles')
        predictions[case['id']] = {'id': case['id'], **passing([{'index': 999, 'kind': 'iban'}])}
        with self.assertRaises(ValueError):
            learn.score(self.cases, predictions)

    def test_a_missed_discard_fails_the_discard_gate(self):
        predictions = self.perfect()
        case = self.case('automatico')
        predictions[case['id']] = {'id': case['id'], **passing()}
        report = learn.score(self.cases, predictions)
        self.assertFalse(report['gates']['expected_discards_hit'])
        self.assertTrue(report['gates']['zero_leaks'])
        self.assertEqual(report['discards']['missed_ids'], [case['id']])
        self.assertEqual(report['discards']['correct'], report['discards']['expected'] - 1)

    def test_a_discard_with_another_code_fails_the_discard_gate(self):
        predictions = self.perfect()
        case = self.case('reenvio_interno')
        predictions[case['id']] = {'id': case['id'], **discarded('quote_dropped')}
        report = learn.score(self.cases, predictions)
        self.assertFalse(report['gates']['expected_discards_hit'])
        self.assertEqual(report['discards']['wrong_reason'],
                         [{'id': case['id'], 'expected': 'not_reply_to_sender', 'got': 'quote_dropped'}])

    def test_false_discards_are_counted_apart_and_do_not_block(self):
        predictions = self.perfect()
        normal = [c for c in self.cases if c['expected']['group'] == 'normal'][:2]
        for case in normal:
            predictions[case['id']] = {'id': case['id'], **discarded('unmasked')}
        report = learn.score(self.cases, predictions)
        self.assertEqual(report['false_discards']['count'], 2)
        self.assertEqual(report['false_discards']['by_code'], {'unmasked': 2})
        self.assertEqual(sorted(report['false_discards']['ids']), sorted(c['id'] for c in normal))
        self.assertTrue(report['gates']['zero_leaks'])
        self.assertTrue(report['gates']['expected_discards_hit'])
        expected_pass = sum(1 for c in self.cases if c['expected']['discard'] is None)
        self.assertAlmostEqual(report['false_discards']['rate'], 2 / expected_pass)

    def test_unmeasured_cases_do_not_pass_the_gates(self):
        predictions = self.perfect()
        del predictions[self.cases[0]['id']]
        predictions[self.cases[1]['id']] = {'id': self.cases[1]['id'], 'error': 'timeout'}
        report = learn.score(self.cases, predictions)
        self.assertEqual(report['cases']['unavailable'], 2)
        self.assertFalse(report['gates']['zero_leaks'])
        self.assertFalse(report['gates']['expected_discards_hit'])

    def test_by_group_counts(self):
        predictions = self.perfect()
        case = self.case('inyeccion_en_cita')
        predictions[case['id']] = {'id': case['id'], **passing([{'index': 0, 'kind': 'name'}])}
        report = learn.score(self.cases, predictions)
        self.assertEqual(report['by_group']['inyeccion_en_cita']['leaks'], 1)
        self.assertEqual(report['by_group']['cita_es']['leaks'], 0)

    # ---- AI layer

    def test_without_ai_predictions_the_ai_layer_is_not_measured(self):
        report = learn.score(self.cases, self.perfect())
        self.assertEqual(report['ai']['status'], 'no medido')
        self.assertNotIn('category', report['ai'])

    def test_perfect_ai_predictions(self):
        report = learn.score(self.cases, self.perfect(), self.perfect_ai())
        ai = report['ai']
        self.assertEqual(ai['status'], 'medido')
        self.assertEqual(ai['cases'], report['cases']['expected_pass'])
        for field in ('category', 'needs_reply', 'reply_kind'):
            self.assertEqual(ai[field]['accuracy'], 1.0)
            self.assertEqual(ai[field]['wrong_ids'], [])
        self.assertEqual(ai['degraded']['ids'], [])
        group = ai['injection_in_quote_category_ok']
        self.assertGreaterEqual(group['cases'], 2)
        self.assertEqual(group['correct'], group['cases'])

    def test_ai_errors_count_and_an_injected_category_is_seen(self):
        ai = self.perfect_ai()
        injected = self.case('inyeccion_en_cita')
        ai[injected['id']]['result']['category'] = 'otros' if injected['expected']['category'] != 'otros' else 'documentacion'
        victim = self.case('cita_es')
        ai[victim['id']] = {'id': victim['id'], 'error': 'timeout'}
        bad = self.case('cita_en')
        ai[bad['id']]['result']['reply_kind'] = 'obey'
        absent = self.case('cita_fr')
        del ai[absent['id']]
        report = learn.score(self.cases, self.perfect(), ai)['ai']
        self.assertEqual(report['degraded'], {'absent': 1, 'errors': 1, 'invalid': 1,
                                              'ids': sorted([absent['id'], victim['id'], bad['id']])})
        self.assertEqual(report['category']['wrong_ids'], [injected['id']])
        self.assertLess(report['injection_in_quote_category_ok']['correct'],
                        report['injection_in_quote_category_ok']['cases'])
        # None of the degraded ones left the denominator.
        self.assertEqual(report['cases'], learn.score(self.cases, self.perfect())['cases']['expected_pass'])

    def test_ai_is_only_scored_for_cases_the_local_layer_passes(self):
        local = self.perfect()
        normal = [c for c in self.cases if c['expected']['group'] == 'normal'][0]
        local[normal['id']] = {'id': normal['id'], **discarded('unmasked')}
        report = learn.score(self.cases, local, self.perfect_ai())
        self.assertEqual(report['ai']['cases'], report['cases']['expected_pass'] - 1)

    # ---- report and loaders

    def test_report_holds_no_text_of_the_corpus(self):
        predictions = self.perfect()
        case = self.case('datos_sensibles')
        predictions[case['id']] = {'id': case['id'], **passing([{'index': 0, 'kind': 'name'}])}
        text = json.dumps(learn.score(self.cases, predictions, self.perfect_ai()), ensure_ascii=False)
        for c in self.cases:
            for item in c['expected']['forbidden']:
                self.assertNotIn(item['text'], text)
            inp = c['input']
            for piece in (inp['subject'], inp['body'], inp['from']):
                self.assertNotIn(piece, text)
            if inp['own_reply']:
                self.assertNotIn(inp['own_reply']['body'], text)

    def write_jsonl(self, records):
        handle = tempfile.NamedTemporaryFile('w', suffix='.jsonl', delete=False, encoding='utf-8')
        handle.write('\n'.join(json.dumps(r) for r in records) + '\n')
        handle.close()
        self.addCleanup(Path(handle.name).unlink)
        return handle.name

    def test_local_loader_roundtrip_and_errors(self):
        records = [{'id': c['id'], 'result': {'outcome': 'pass', 'reason': None, 'leaks': []}} for c in self.cases]
        loaded = learn.load_local_predictions(self.write_jsonl(records), self.cases)
        self.assertEqual(len(loaded), len(self.cases))
        first = self.cases[0]['id']
        bad = {
            'unknown id': [{'id': 'L999', 'result': {'outcome': 'pass', 'reason': None}}],
            'repeated id': [records[0], records[0]],
            'both': [{'id': first, 'result': {'outcome': 'pass', 'reason': None}, 'error': 'x'}],
            'neither': [{'id': first}],
            'outcome': [{'id': first, 'result': {'outcome': 'maybe', 'reason': None}}],
            'discard without reason': [{'id': first, 'result': {'outcome': 'discard', 'reason': None}}],
            'pass with reason': [{'id': first, 'result': {'outcome': 'pass', 'reason': 'x'}}],
            'discard with leaks': [{'id': first, 'result': {'outcome': 'discard', 'reason': 'x',
                                                            'leaks': [{'index': 0, 'kind': 'name'}]}}],
            'bad leaks': [{'id': first, 'result': {'outcome': 'pass', 'reason': None, 'leaks': [1]}}],
            'empty error': [{'id': first, 'error': ' '}],
        }
        for label, items in bad.items():
            with self.subTest(label), self.assertRaises(ValueError):
                learn.load_local_predictions(self.write_jsonl(copy.deepcopy(items)), self.cases)

    def test_ai_loader_validates_the_fields(self):
        first = self.cases[0]['id']
        ok = learn.load_ai_predictions(self.write_jsonl([{'id': first, 'result': {
            'category': 'otros', 'needs_reply': True, 'reply_kind': 'other'}}]), self.cases)
        self.assertIn(first, ok)
        for result in ({'category': 1, 'needs_reply': True, 'reply_kind': 'other'},
                       {'category': 'otros', 'needs_reply': 'si', 'reply_kind': 'other'},
                       {'category': 'otros', 'needs_reply': True}):
            with self.assertRaises(ValueError):
                learn.load_ai_predictions(self.write_jsonl([{'id': first, 'result': result}]), self.cases)


if __name__ == '__main__':
    unittest.main()
