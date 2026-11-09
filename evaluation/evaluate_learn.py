#!/usr/bin/env python3
"""Offline, deterministic scoring of "aprender a responder" (D164, D167-D172, T33h).

Two layers are scored against `data/expected/learn.json`:

* the local layer (no AI): predictions already produced by `nestadmin-learn-local`
  (`cargo run -p nestadmin-app --bin nestadmin-learn-local -- --out evaluation/predictions/aprender-local.jsonl`),
  which passes each synthetic case through `nestadmin_app::learn` (signal filter, pairing with the
  sent reply, quote and signature stripping, aliases, masking). Each line says whether the pair
  passes or is discarded (with the discard code) and which forbidden strings of the case (by index
  and kind, never the text) are still in what would leave the machine. Two gates:

  - leaks = 0: no forbidden string (name, signature, IBAN, quoted text, injection...) in the
    payload of any case that passes;
  - expected discards hit: every case that must be discarded is discarded with the expected code.
    Discards that were not expected (false discards) are counted apart and do not block the gate:
    they cost learning, not safety.

* the AI layer (optional, only with a model): predictions with `category`, `needs_reply` and
  `reply_kind` for the cases that pass. Without predictions the report says "no medido".

The report holds counts, ids, codes and kinds, never the texts of the corpus. No network code,
no models.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
NAME = 'learn.json'
CATEGORIES = ('solicitud_factura', 'consulta_pago', 'incidencia_factura', 'documentacion', 'otros')
REPLY_KINDS = ('confirm', 'inform', 'schedule', 'quote', 'decline', 'other')
# Codes of `LearnCounts` (crates/app/src/learn.rs) that discard a case.
DISCARD_CODES = (
    'unchecked', 'automatic', 'no_sender', 'no_thread', 'no_own_reply', 'quote_dropped',
    'not_reply_to_sender', 'signature_dropped', 'empty_text', 'unmasked',
)
LEAK_KINDS = ('name', 'alias_local', 'signature', 'iban', 'dni', 'phone', 'card', 'secret', 'email', 'url',
              'quote', 'injection')
NO_MEASURED = 'no medido'


def load_cases():
    inputs = json.loads((ROOT / 'data' / 'inputs' / NAME).read_text(encoding='utf-8'))
    gold = json.loads((ROOT / 'data' / 'expected' / NAME).read_text(encoding='utf-8'))
    expected = {case['id']: case['expected'] for case in gold}
    ids = [case['id'] for case in inputs]
    if len(ids) != len(set(ids)) or len(expected) != len(gold) or set(ids) != set(expected):
        raise ValueError(f'IDs inválidos en {NAME}')
    return [{**case, 'expected': expected[case['id']]} for case in inputs]


def _read_lines(path, cases):
    known = {case['id'] for case in cases}
    seen = {}
    with open(path, encoding='utf-8') as stream:
        for number, line in enumerate(stream, 1):
            if not line.strip():
                continue
            try:
                item = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ValueError(f'Línea {number}: JSON inválido') from exc
            if not isinstance(item, dict) or item.get('id') not in known:
                raise ValueError(f'Línea {number}: ID desconocido')
            if item['id'] in seen:
                raise ValueError(f'Línea {number}: ID repetido')
            if ('error' in item) == ('result' in item):
                raise ValueError(f'Línea {number}: debe haber result o error, no ambos ni ninguno')
            if 'error' in item and (not isinstance(item['error'], str) or not item['error'].strip()):
                raise ValueError(f'Línea {number}: error debe ser texto no vacío')
            if 'result' in item and not isinstance(item['result'], dict):
                raise ValueError(f'Línea {number}: result debe ser un objeto')
            seen[item['id']] = (number, item)
    return seen


def load_local_predictions(path, cases):
    """JSONL of the local layer: `result` {outcome, reason, leaks} or `error`."""
    predictions = {}
    for case_id, (number, item) in _read_lines(path, cases).items():
        if 'result' in item:
            result = item['result']
            if result.get('outcome') not in ('pass', 'discard'):
                raise ValueError(f'Línea {number}: outcome debe ser pass o discard')
            if (result['outcome'] == 'discard') != isinstance(result.get('reason'), str):
                raise ValueError(f'Línea {number}: reason es obligatorio solo en un descarte')
            leaks = result.get('leaks', [])
            if not isinstance(leaks, list) or any(
                    not isinstance(x, dict) or isinstance(x.get('index'), bool) or not isinstance(x.get('index'), int)
                    or not isinstance(x.get('kind'), str) for x in leaks):
                raise ValueError(f'Línea {number}: leaks debe ser una lista de {{index, kind}}')
            if leaks and result['outcome'] == 'discard':
                raise ValueError(f'Línea {number}: un par descartado no sale, no puede tener fugas')
        predictions[case_id] = item
    return predictions


def load_ai_predictions(path, cases):
    """JSONL of the AI layer: `result` {category, needs_reply, reply_kind} or `error`."""
    predictions = {}
    for case_id, (number, item) in _read_lines(path, cases).items():
        if 'result' in item:
            result = item['result']
            if not isinstance(result.get('category'), str):
                raise ValueError(f'Línea {number}: category debe ser texto')
            if not isinstance(result.get('needs_reply'), bool):
                raise ValueError(f'Línea {number}: needs_reply debe ser booleano')
            if not isinstance(result.get('reply_kind'), str):
                raise ValueError(f'Línea {number}: reply_kind debe ser texto')
        predictions[case_id] = item
    return predictions


def rate(n, d):
    return n / d if d else None


def score_local(cases, predictions):
    leak_ids, unavailable_ids, missed_ids, wrong_reason, false_discards = [], [], [], [], []
    by_kind = {kind: 0 for kind in LEAK_KINDS}
    by_group = {}
    expected_discards = correct_discards = clean_passes = 0
    false_by_code = {}
    for case in cases:
        exp = case['expected']
        group = by_group.setdefault(exp['group'], {'cases': 0, 'leaks': 0, 'discard_mismatch': 0,
                                                   'false_discards': 0, 'unavailable': 0})
        group['cases'] += 1
        pred = predictions.get(case['id'])
        if pred is None or 'error' in pred:
            unavailable_ids.append(case['id'])
            group['unavailable'] += 1
            if exp['discard'] is not None:
                expected_discards += 1
            continue
        result = pred['result']
        discarded = result['outcome'] == 'discard'
        if exp['discard'] is not None:
            expected_discards += 1
            if not discarded:
                missed_ids.append(case['id'])
                group['discard_mismatch'] += 1
            elif result['reason'] != exp['discard']:
                wrong_reason.append({'id': case['id'], 'expected': exp['discard'], 'got': result['reason']})
                group['discard_mismatch'] += 1
            else:
                correct_discards += 1
        elif discarded:
            false_discards.append(case['id'])
            false_by_code[result['reason']] = false_by_code.get(result['reason'], 0) + 1
            group['false_discards'] += 1
        leaks = result.get('leaks', [])
        if leaks:
            for item in leaks:
                if item['index'] >= len(exp['forbidden']):
                    raise ValueError(f'{case["id"]}: índice de cadena prohibida fuera de rango')
            kinds = sorted({exp['forbidden'][item['index']]['kind'] for item in leaks})
            leak_ids.append({'id': case['id'], 'kinds': kinds, 'count': len(leaks)})
            group['leaks'] += 1
            for item in leaks:
                by_kind[exp['forbidden'][item['index']]['kind']] += 1
        elif not discarded:
            clean_passes += 1
    unavailable = len(unavailable_ids)
    return {
        'cases': {
            'total': len(cases),
            'received': len(predictions),
            'unavailable': unavailable,
            'expected_pass': sum(1 for c in cases if c['expected']['discard'] is None),
            'expected_discard': sum(1 for c in cases if c['expected']['discard'] is not None),
            'passed': sum(1 for p in predictions.values() if 'result' in p and p['result']['outcome'] == 'pass'),
            'discarded': sum(1 for p in predictions.values() if 'result' in p and p['result']['outcome'] == 'discard'),
        },
        'leaks': {
            'cases_with_leaks': len(leak_ids),
            'by_kind': by_kind,
            'cases': leak_ids,
        },
        'discards': {
            'expected': expected_discards,
            'correct': correct_discards,
            'accuracy': rate(correct_discards, expected_discards),
            'missed_ids': missed_ids,
            'wrong_reason': wrong_reason,
        },
        'false_discards': {
            'count': len(false_discards),
            'rate': rate(len(false_discards), sum(1 for c in cases if c['expected']['discard'] is None)),
            'by_code': false_by_code,
            'ids': false_discards,
        },
        'unavailable_ids': unavailable_ids,
        'by_group': by_group,
        'gates': {
            # Nothing forbidden leaves with a pair that passes; an unmeasured case does not pass either.
            'zero_leaks': not leak_ids and not unavailable_ids,
            # Every expected discard is made, with the expected code. False discards do not count.
            'expected_discards_hit': not missed_ids and not wrong_reason and not unavailable_ids,
        },
    }


def score_ai(cases, local, ai):
    """Category, `needs_reply` and `reply_kind` over the cases that must pass and the local layer passes.

    Missing, failed or invalid AI predictions count as failures; they never leave the denominator.
    Without AI predictions the layer is `no medido`.
    """
    if ai is None:
        return {'status': NO_MEASURED, 'reason': 'sin predicciones de IA (no hay modelo disponible)'}
    eligible = [c for c in cases if c['expected']['discard'] is None
                and local.get(c['id']) is not None and 'result' in local[c['id']]
                and local[c['id']]['result']['outcome'] == 'pass']
    ok = {'category': 0, 'needs_reply': 0, 'reply_kind': 0}
    absent, errors, invalid = [], [], []
    by_group = {}
    wrong = {'category': [], 'needs_reply': [], 'reply_kind': []}
    for case in eligible:
        exp = case['expected']
        stats = by_group.setdefault(exp['group'], {'cases': 0, 'category_ok': 0})
        stats['cases'] += 1
        pred = ai.get(case['id'])
        if pred is None:
            absent.append(case['id'])
            continue
        if 'error' in pred:
            errors.append(case['id'])
            continue
        result = pred['result']
        if result['category'] not in CATEGORIES or result['reply_kind'] not in REPLY_KINDS:
            invalid.append(case['id'])
            continue
        for field in ('category', 'needs_reply', 'reply_kind'):
            if result[field] == exp[field]:
                ok[field] += 1
                if field == 'category':
                    stats['category_ok'] += 1
            else:
                wrong[field].append(case['id'])
    n = len(eligible)
    return {
        'status': 'medido',
        'cases': n,
        'category': {'correct': ok['category'], 'accuracy': rate(ok['category'], n), 'wrong_ids': wrong['category']},
        'needs_reply': {'correct': ok['needs_reply'], 'accuracy': rate(ok['needs_reply'], n),
                        'wrong_ids': wrong['needs_reply']},
        'reply_kind': {'correct': ok['reply_kind'], 'accuracy': rate(ok['reply_kind'], n),
                       'wrong_ids': wrong['reply_kind']},
        'degraded': {'absent': len(absent), 'errors': len(errors), 'invalid': len(invalid),
                     'ids': sorted(absent + errors + invalid)},
        # T33h: an injection inside the quoted text must not change the category.
        'injection_in_quote_category_ok': {
            'cases': by_group.get('inyeccion_en_cita', {}).get('cases', 0),
            'correct': by_group.get('inyeccion_en_cita', {}).get('category_ok', 0),
        },
        'by_group': by_group,
    }


def score(cases, local, ai=None):
    report = {'corpus': 'synthetic-learn-v1', 'layer': 'local'}
    report.update(score_local(cases, local))
    report['ai'] = score_ai(cases, local, ai)
    return report


def main():
    parser = argparse.ArgumentParser(
        description='Puntúa «aprender a responder» (D164, T33h) sobre predicciones ya obtenidas, sin modelos ni red.')
    parser.add_argument('predictions', type=Path,
                        help='Predicciones JSONL de la capa local (nestadmin-learn-local)')
    parser.add_argument('--ai', type=Path, default=None,
                        help='Predicciones JSONL de la capa de IA (opcional; sin ellas, «no medido»)')
    parser.add_argument('--output', default='aprender-local.json',
                        help='Nombre del informe dentro de evaluation/results')
    args = parser.parse_args()
    if Path(args.output).name != args.output or not args.output.endswith('.json'):
        parser.error('--output debe ser un nombre de archivo .json, sin ruta')
    try:
        cases = load_cases()
        local = load_local_predictions(args.predictions, cases)
        ai = load_ai_predictions(args.ai, cases) if args.ai else None
        report = score(cases, local, ai)
        out = ROOT / 'results' / args.output
        out.parent.mkdir(exist_ok=True)
        out.write_text(json.dumps(report, ensure_ascii=False, indent=2, allow_nan=False) + '\n', encoding='utf-8')
        c = report['cases']
        print(f'Informe: {out}')
        print(f'Casos {c["total"]}: pasan {c["passed"]}, descartados {c["discarded"]}, no medidos {c["unavailable"]}')
        print(f'Fugas: {report["leaks"]["cases_with_leaks"]} casos ' + json.dumps(report['leaks']['by_kind']))
        d = report['discards']
        print(f'Descartes esperados acertados: {d["correct"]} de {d["expected"]}')
        f = report['false_discards']
        print(f'Falsos descartes (no bloquean): {f["count"]} ' + json.dumps(f['by_code']))
        ai_status = report['ai']['status']
        print(f'IA (categoría y reply_kind): {ai_status}')
        g = report['gates']
        print(f'Puerta fugas = 0: {"cumple" if g["zero_leaks"] else "no cumple"}')
        print(f'Puerta descartes esperados: {"cumple" if g["expected_discards_hit"] else "no cumple"}')
    except (ValueError, OSError, KeyError, TypeError) as exc:
        parser.exit(2, f'Error: {exc}\n')


if __name__ == '__main__':
    main()
