#!/usr/bin/env python3
"""Offline, deterministic scoring for the synthetic evaluation set."""
from __future__ import annotations

import argparse
import json
import math
import statistics
from pathlib import Path

ROOT = Path(__file__).resolve().parent
FIELDS = ('supplier', 'invoice_number', 'date', 'subtotal', 'tax', 'total')


def load_cases():
    cases = []
    for name in ('invoices.json', 'emails.json'):
        inputs = json.loads((ROOT / 'data' / 'inputs' / name).read_text(encoding='utf-8'))
        gold = json.loads((ROOT / 'data' / 'expected' / name).read_text(encoding='utf-8'))
        expected = {case['id']: case['expected'] for case in gold}
        ids = [case['id'] for case in inputs]
        if len(ids) != len(set(ids)) or len(expected) != len(gold) or set(ids) != set(expected):
            raise ValueError(f'IDs inválidos en {name}')
        cases.extend({**case, 'expected': expected[case['id']]} for case in inputs)
    ids = [case['id'] for case in cases]
    if len(ids) != len(set(ids)) or len(cases) != 30:
        raise ValueError('Se requieren exactamente 30 identificadores únicos')
    return cases


def load_predictions(path, cases):
    known = {case['id'] for case in cases}
    predictions = {}
    with open(path, encoding='utf-8') as stream:
        for line_number, line in enumerate(stream, 1):
            if not line.strip():
                continue
            try:
                item = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ValueError(f'Línea {line_number}: JSON inválido') from exc
            if not isinstance(item, dict) or item.get('id') not in known:
                raise ValueError(f'Línea {line_number}: ID desconocido')
            if item['id'] in predictions:
                raise ValueError(f'Línea {line_number}: ID repetido')
            for key in ('latency_ms', 'peak_memory_bytes'):
                value = item.get(key)
                if value is not None and (isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0):
                    raise ValueError(f'Línea {line_number}: {key} debe ser un número finito no negativo')
            if 'error' in item and (not isinstance(item['error'], str) or not item['error'].strip()):
                raise ValueError(f'Línea {line_number}: error debe ser texto no vacío')
            if 'result' in item and not isinstance(item['result'], dict):
                raise ValueError(f'Línea {line_number}: result debe ser un objeto')
            if 'error' not in item and 'result' not in item:
                raise ValueError(f'Línea {line_number}: falta result o error')
            if 'error' in item and 'result' in item:
                raise ValueError(f'Línea {line_number}: no se admite result y error a la vez')
            result = item.get('result', {})
            if 'fields' in result and (not isinstance(result['fields'], dict) or any(v is not None and not isinstance(v, str) for v in result['fields'].values())):
                raise ValueError(f'Línea {line_number}: fields debe contener cadenas o null')
            if 'issues' in result and (not isinstance(result['issues'], list) or any(not isinstance(v, str) for v in result['issues'])):
                raise ValueError(f'Línea {line_number}: issues debe ser una lista de cadenas')
            if any(key in result and not isinstance(result[key], str) for key in ('category', 'draft')):
                raise ValueError(f'Línea {line_number}: category y draft deben ser texto')
            predictions[item['id']] = item
    return predictions


def norm(value):
    return value.strip().casefold() if isinstance(value, str) else value


def rate(n, d):
    return n / d if d else None


def score(cases, predictions):
    field_correct = field_total = hallucinated = absent_total = 0
    issue_tp = issue_fp = issue_fn = 0
    category_correct = category_total = drafts_passed = drafts_total = 0
    missing = errors = 0
    latencies, memories, details = [], [], []
    for case in cases:
        prediction = predictions.get(case['id'])
        status = 'ok'
        if prediction is None:
            missing += 1
            status = 'missing'
            prediction = {}
        elif 'error' in prediction:
            errors += 1
            status = 'error'
        for key, accumulator in (('latency_ms', latencies), ('peak_memory_bytes', memories)):
            if prediction.get(key) is not None:
                accumulator.append(prediction[key])
        result = prediction.get('result', {})
        exp = case['expected']
        detail = {'id': case['id'], 'status': status}
        if case['kind'] == 'invoice':
            actual_fields = result.get('fields', {})
            correct = 0
            for field in FIELDS:
                expected = exp['fields'][field]
                actual = actual_fields.get(field)
                field_total += 1
                # Missing predictions cannot score as correct, even for an expected null.
                matched = status == 'ok' and field in actual_fields and norm(actual) == norm(expected)
                field_correct += int(matched)
                correct += int(matched)
                if expected is None:
                    absent_total += 1
                    if actual is not None and norm(actual) != '':
                        hallucinated += 1
            expected_issues, actual_issues = set(exp['issues']), set(result.get('issues', []))
            issue_tp += len(expected_issues & actual_issues)
            issue_fp += len(actual_issues - expected_issues)
            issue_fn += len(expected_issues - actual_issues)
            detail['fields_correct'] = correct
        else:
            category_total += 1
            category_match = status == 'ok' and result.get('category') == exp['category']
            category_correct += int(category_match)
            draft = norm(result.get('draft', '')) or ''
            rubric = exp['draft_rubric']
            draft_ok = status == 'ok' and bool(draft) and any(norm(word) in draft for word in rubric['must_include_any']) and not any(norm(word) in draft for word in rubric['must_not_include'])
            drafts_total += 1
            drafts_passed += int(draft_ok)
            detail.update(category_correct=category_match, draft_heuristic_passed=draft_ok)
        details.append(detail)
    return {
        'cases': {'total': len(cases), 'received': len(predictions), 'missing': missing, 'errors': errors},
        'extraction': {'correct_fields': field_correct, 'total_fields': field_total, 'accuracy': rate(field_correct, field_total), 'hallucinated_absent_fields': hallucinated, 'expected_absent_fields': absent_total},
        'issues': {'true_positive': issue_tp, 'false_positive': issue_fp, 'false_negative': issue_fn, 'precision': rate(issue_tp, issue_tp + issue_fp), 'recall': rate(issue_tp, issue_tp + issue_fn)},
        'classification': {'correct': category_correct, 'total': category_total, 'accuracy': rate(category_correct, category_total)},
        'draft_heuristic': {'passed': drafts_passed, 'total': drafts_total, 'rate': rate(drafts_passed, drafts_total)},
        'performance': {'latency_ms_mean': statistics.mean(latencies) if latencies else None, 'latency_samples': len(latencies), 'peak_memory_bytes_max': max(memories) if memories else None, 'memory_samples': len(memories)},
        'details': details,
    }


def main():
    parser = argparse.ArgumentParser(description='Puntúa salidas sintéticas existentes, sin ejecutar modelos ni usar la red.')
    parser.add_argument('predictions', type=Path, help='Predicciones JSONL para el primer proveedor')
    parser.add_argument('--compare', type=Path, help='Predicciones JSONL del segundo proveedor sobre el mismo corpus')
    parser.add_argument('--output', default='report.json', help='Nombre del informe dentro de evaluation/results')
    args = parser.parse_args()
    if Path(args.output).name != args.output or not args.output.endswith('.json'):
        parser.error('--output debe ser un nombre de archivo .json, sin ruta')
    try:
        cases = load_cases()
        report = {'corpus': 'synthetic-v1', 'first': score(cases, load_predictions(args.predictions, cases))}
        if args.compare:
            report['second'] = score(cases, load_predictions(args.compare, cases))
        out = ROOT / 'results' / args.output
        out.parent.mkdir(exist_ok=True)
        out.write_text(json.dumps(report, ensure_ascii=False, indent=2, allow_nan=False) + '\n', encoding='utf-8')
        print(f'Informe: {out}')
    except (ValueError, OSError, KeyError, TypeError) as exc:
        parser.exit(2, f'Error: {exc}\n')


if __name__ == '__main__':
    main()
