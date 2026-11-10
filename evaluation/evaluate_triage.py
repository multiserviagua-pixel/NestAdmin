#!/usr/bin/env python3
"""Offline, deterministic scoring of mail triage (D157, D158, D164) on the synthetic corpus.

Two layers are scored against `data/expected/triage.json`:

* the local layer (D157): the route (`informativo`, `rebote`, `ia`) derived from the sender
  address and the headers, with `hidden_fn` (a mail that needs a reply and the local layer
  sends to Informativos). It needs no predictions: `--local-only`.
* the AI predictions already produced (for example with `nestadmin-predict --set triaje`):
  category, `needs_reply`, draft rubric and degraded results. Missing or failed predictions
  count as failures; they never leave the denominator.

The local layer is re-implemented here from `crates/core/src/mail_signals.rs`. Parity is
enforced against the hand-labelled routes of the corpus, from both sides: the Rust test
`crates/core/tests/triage_corpus.rs` and `evaluation/tests/test_evaluate_triage.py` check their
implementation against the same labels. The module has no network code and runs no model.
"""
from __future__ import annotations

import argparse
import json
import math
import statistics
from pathlib import Path

ROOT = Path(__file__).resolve().parent
NAME = 'triage.json'
CATEGORIES = ('solicitud_factura', 'consulta_pago', 'incidencia_factura', 'documentacion', 'presupuesto', 'pedido', 'cita',
              'informacion', 'acuse', 'reclamacion', 'otros')
ROUTES = ('informativo', 'rebote', 'ia')
# Provisional internal gates (D164); not promises.
GATE_NEEDS_REPLY_RECALL = 0.95

NOREPLY_FORMS = (
    'noreply', 'no-reply', 'no_reply', 'no.reply', 'noresponder', 'no-responder', 'no_responder',
    'donotreply', 'do-not-reply', 'do_not_reply', 'do.not.reply',
)


# ---------------------------------------------------------------- local layer (D157)

def local_part(sender):
    if not sender:
        return None
    start, end = sender.rfind('<'), sender.rfind('>')
    text = sender[start + 1:end] if 0 <= start < end else sender
    text = text.strip().strip('"').strip()
    local, at, domain = text.rpartition('@')
    if not at or not local or not domain:
        return None
    local = local.strip().lower().split('+', 1)[0]
    return local or None


def is_noreply(local):
    def separator(ch):
        return ch is None or ch in '-_.'

    for form in NOREPLY_FORMS:
        pos = local.find(form)
        while pos != -1:
            before = local[pos - 1] if pos > 0 else None
            after_at = pos + len(form)
            after = local[after_at] if after_at < len(local) else None
            if separator(before) and separator(after):
                return True
            pos = local.find(form, after_at)
    return False


def clean_value(value):
    out, depth = [], 0
    for ch in value:
        if ch == '(':
            depth += 1
        elif ch == ')':
            depth = max(depth - 1, 0)
        elif ch == ';' and depth == 0:
            break
        elif depth == 0:
            out.append(ch)
    return ''.join(out).strip().lower()


def signals(sender, headers):
    """Signal codes, like `mail_signals::detect`. `headers` is a list of (name, value) or None."""
    found = set()
    local = local_part(sender)
    if local is not None:
        if is_noreply(local):
            found.add('noreply_address')
        if local == 'mailer-daemon':
            found.add('bounce_address')
    for name, value in headers or []:
        name = name.strip().lower()
        if name == 'auto-submitted':
            v = clean_value(value)
            if v and v != 'no':
                found.add('auto_submitted')
        elif name == 'precedence':
            v = clean_value(value)
            if v in ('junk', 'bulk', 'list'):
                found.add('precedence_' + v)
        elif name == 'list-id':
            if value.strip():
                found.add('list_id')
        elif name == 'list-unsubscribe':
            if value.strip():
                found.add('list_unsubscribe')
        elif name == 'return-path':
            if ''.join(value.split()) == '<>':
                found.add('null_return_path')
    return sorted(found)


STRONG = {'noreply_address', 'auto_submitted', 'precedence_junk'}
BOUNCE = {'bounce_address', 'null_return_path'}


def local_route(sender, headers):
    """Bounce before strong signal; a weak signal alone does not hide the mail."""
    found = set(signals(sender, headers))
    if found & BOUNCE:
        return 'rebote'
    if found & STRONG:
        return 'informativo'
    return 'ia'


def case_headers(case):
    return [(h['name'], h['value']) for h in case['input']['headers']]


# ---------------------------------------------------------------- corpus and predictions

def load_cases():
    inputs = json.loads((ROOT / 'data' / 'inputs' / NAME).read_text(encoding='utf-8'))
    gold = json.loads((ROOT / 'data' / 'expected' / NAME).read_text(encoding='utf-8'))
    expected = {case['id']: case['expected'] for case in gold}
    ids = [case['id'] for case in inputs]
    if len(ids) != len(set(ids)) or len(expected) != len(gold) or set(ids) != set(expected):
        raise ValueError(f'IDs inválidos en {NAME}')
    return [{**case, 'expected': expected[case['id']]} for case in inputs]


def load_predictions(path, cases):
    """JSONL like the other evaluators: `result` or `error`; unknown or repeated IDs fail."""
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
            value = item.get('latency_ms')
            if value is not None and (isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0):
                raise ValueError(f'Línea {line_number}: latency_ms debe ser un número finito no negativo')
            if ('error' in item) == ('result' in item):
                raise ValueError(f'Línea {line_number}: debe haber result o error, no ambos ni ninguno')
            if 'error' in item and (not isinstance(item['error'], str) or not item['error'].strip()):
                raise ValueError(f'Línea {line_number}: error debe ser texto no vacío')
            if 'result' in item:
                result = item['result']
                if not isinstance(result, dict):
                    raise ValueError(f'Línea {line_number}: result debe ser un objeto')
                if not isinstance(result.get('category'), str):
                    raise ValueError(f'Línea {line_number}: category debe ser texto')
                if not isinstance(result.get('needs_reply'), bool):
                    raise ValueError(f'Línea {line_number}: needs_reply debe ser booleano')
                if 'draft' in result and not isinstance(result['draft'], str):
                    raise ValueError(f'Línea {line_number}: draft debe ser texto')
                if 'draft_error' in result and not isinstance(result['draft_error'], str):
                    raise ValueError(f'Línea {line_number}: draft_error debe ser texto')
            predictions[item['id']] = item
    return predictions


def rate(n, d):
    return n / d if d else None


def norm(value):
    return value.strip().casefold() if isinstance(value, str) else ''


# ---------------------------------------------------------------- scoring

def score_local(cases):
    confusion = {e: {a: 0 for a in ROUTES} for e in ROUTES}
    correct = 0
    hidden_fn, hidden_ids = 0, []
    review_noise = 0  # no reply needed and the local layer does not hide it (cost only)
    for case in cases:
        exp = case['expected']
        actual = local_route(case['input']['from'], case_headers(case))
        confusion[exp['route']][actual] += 1
        correct += int(actual == exp['route'])
        if exp['needs_reply'] and actual == 'informativo':
            hidden_fn += 1
            hidden_ids.append(case['id'])
        if not exp['needs_reply'] and actual == 'ia':
            review_noise += 1
    return {
        'route_correct': correct, 'total': len(cases), 'route_accuracy': rate(correct, len(cases)),
        'confusion': confusion,
        'hidden_fn': hidden_fn, 'hidden_fn_ids': hidden_ids,
        'no_reply_needed_left_in_review': review_noise,
    }


def prf(tp, fp, fn):
    return {'true_positive': tp, 'false_positive': fp, 'false_negative': fn,
            'precision': rate(tp, tp + fp), 'recall': rate(tp, tp + fn)}


def score_ai(cases, predictions):
    missing = errors = draft_errors = 0
    latencies, details = [], []
    cat = {c: [0, 0, 0] for c in CATEGORIES}  # tp, fp, fn
    cat_labels = list(CATEGORIES) + ['missing_or_error']
    confusion = {e: {a: 0 for a in cat_labels + ['unknown']} for e in CATEGORIES}
    nr = {True: [0, 0, 0], False: [0, 0, 0]}
    nr_confusion = {'expected_true': {'pred_true': 0, 'pred_false': 0, 'missing_or_error': 0},
                    'expected_false': {'pred_true': 0, 'pred_false': 0, 'missing_or_error': 0}}
    rubric_ok = rubric_total = 0
    for case in cases:
        exp = case['expected']
        pred = predictions.get(case['id'])
        if pred is None:
            status, result = 'missing', {}
            missing += 1
        elif 'error' in pred:
            status, result = 'error', {}
            errors += 1
        else:
            status, result = 'ok', pred['result']
        if pred is not None and pred.get('latency_ms') is not None:
            latencies.append(pred['latency_ms'])
        ok = status == 'ok'
        detail = {'id': case['id'], 'status': status, 'group': exp.get('group')}
        # needs_reply, both classes: an absent or failed prediction is a miss for the expected class.
        expected_nr = exp['needs_reply']
        row = nr_confusion['expected_true' if expected_nr else 'expected_false']
        if ok:
            predicted_nr = result['needs_reply']
            row['pred_true' if predicted_nr else 'pred_false'] += 1
            for klass in (True, False):
                if predicted_nr == klass and expected_nr == klass:
                    nr[klass][0] += 1
                elif predicted_nr == klass:
                    nr[klass][1] += 1
                elif expected_nr == klass:
                    nr[klass][2] += 1
            detail['needs_reply_correct'] = predicted_nr == expected_nr
        else:
            row['missing_or_error'] += 1
            nr[expected_nr][2] += 1
            detail['needs_reply_correct'] = False
        # Category: only where a reply is needed (the category steers the draft); automatic
        # mail carries a secondary label that is not scored.
        if expected_nr:
            expected_cat = exp['category']
            if ok:
                predicted_cat = result['category']
                confusion[expected_cat][predicted_cat if predicted_cat in CATEGORIES else 'unknown'] += 1
                if predicted_cat == expected_cat:
                    cat[expected_cat][0] += 1
                else:
                    cat[expected_cat][2] += 1
                    if predicted_cat in cat:
                        cat[predicted_cat][1] += 1
                detail['category_correct'] = predicted_cat == expected_cat
            else:
                confusion[expected_cat]['missing_or_error'] += 1
                cat[expected_cat][2] += 1
                detail['category_correct'] = False
            # Draft rubric: a missing draft, a failed draft or a "no reply needed" verdict fails.
            rubric = exp['draft_rubric']
            draft = norm(result.get('draft')) if ok else ''
            passed = (bool(draft)
                      and any(norm(w) in draft for w in rubric['must_include_any'])
                      and not any(norm(w) in draft for w in rubric['must_not_include']))
            rubric_total += 1
            rubric_ok += int(passed)
            detail['draft_rubric_passed'] = passed
        if ok and 'draft_error' in result:
            draft_errors += 1
            detail['draft_error'] = result['draft_error']
        details.append(detail)
    degraded_total = missing + errors + draft_errors
    return {
        'cases': {'total': len(cases), 'received': len(predictions), 'missing': missing, 'errors': errors},
        'degraded': {'missing': missing, 'errors': errors, 'draft_errors': draft_errors,
                     'total': degraded_total, 'rate': rate(degraded_total, len(cases))},
        'category': {'scored_cases': sum(1 for c in cases if c['expected']['needs_reply']),
                     'per_category': {c: prf(*cat[c]) for c in CATEGORIES},
                     'confusion': confusion},
        'needs_reply': {'true': prf(*nr[True]), 'false': prf(*nr[False]), 'confusion': nr_confusion},
        'draft_rubric': {'passed': rubric_ok, 'total': rubric_total, 'rate': rate(rubric_ok, rubric_total)},
        'performance': {'latency_ms_mean': statistics.mean(latencies) if latencies else None,
                        'latency_samples': len(latencies)},
        'details': details,
    }


def gates(local, ai):
    """Provisional gates of D164 for this corpus. `None` when the layer was not measured."""
    out = {'hidden_fn_zero': local['hidden_fn'] == 0}
    if ai is None:
        out['needs_reply_true_recall_at_least_0_95'] = None
    else:
        recall = ai['needs_reply']['true']['recall']
        out['needs_reply_true_recall_at_least_0_95'] = recall is not None and recall >= GATE_NEEDS_REPLY_RECALL
    return out


def score(cases, predictions=None):
    local = score_local(cases)
    ai = score_ai(cases, predictions) if predictions is not None else None
    return {'corpus': 'synthetic-triage-v1', 'local': local, 'ai': ai, 'gates': gates(local, ai)}


def main():
    parser = argparse.ArgumentParser(description='Puntúa el triaje sintético (D164): capa local y predicciones ya obtenidas, sin modelos ni red.')
    parser.add_argument('predictions', nargs='?', type=Path, help='Predicciones JSONL (nestadmin-predict --set triaje)')
    parser.add_argument('--local-only', action='store_true', help='Solo la capa local; no necesita predicciones')
    parser.add_argument('--output', default='triage_report.json', help='Nombre del informe dentro de evaluation/results')
    args = parser.parse_args()
    if Path(args.output).name != args.output or not args.output.endswith('.json'):
        parser.error('--output debe ser un nombre de archivo .json, sin ruta')
    if args.local_only == (args.predictions is not None):
        parser.error('indique las predicciones o --local-only, no ambos ni ninguno')
    try:
        cases = load_cases()
        predictions = None if args.local_only else load_predictions(args.predictions, cases)
        report = score(cases, predictions)
        out = ROOT / 'results' / args.output
        out.parent.mkdir(exist_ok=True)
        out.write_text(json.dumps(report, ensure_ascii=False, indent=2, allow_nan=False) + '\n', encoding='utf-8')
        local = report['local']
        print(f'Informe: {out}')
        print(f'Capa local: ruta correcta {local["route_correct"]}/{local["total"]}; hidden_fn={local["hidden_fn"]}')
        if report['ai'] is not None:
            ai = report['ai']
            print(f'IA: needs_reply verdadero, cobertura={ai["needs_reply"]["true"]["recall"]}; '
                  f'rúbrica={ai["draft_rubric"]["rate"]}; degradados={ai["degraded"]["total"]}')
        print('Puertas D164 (provisionales): ' + json.dumps(report['gates']))
    except (ValueError, OSError, KeyError, TypeError) as exc:
        parser.exit(2, f'Error: {exc}\n')


if __name__ == '__main__':
    main()
