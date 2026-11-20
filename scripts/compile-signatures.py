#!/usr/bin/env python3
"""Validate the canonical compiled profile and generate allocation-free Rust tables.

Requires jsonschema (also installed by check-jsonschema). Never loads research
packs implicitly. No runtime/firmware dependency on Python or JSON parsing.
"""
import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
GROUPS = [
    ('mac_oui', 'MAC_PREFIXES', 'MAC_OUI'),
    ('wifi_ssid_pattern', 'SSID_PATTERNS', 'SSID_PATTERN'),
    ('wifi_ssid_exact', 'SSID_EXACT', 'SSID_EXACT'),
    ('wifi_ssid_contains', 'SSID_KEYWORDS', 'SSID_KEYWORD'),
    ('wifi_name_contains', 'WIFI_NAME_KEYWORDS', 'WIFI_NAME'),
    ('ble_name_contains', 'BLE_NAME_PATTERNS', 'BLE_NAME'),
    ('ble_service_uuid16', 'BLE_SERVICE_UUIDS_16', 'BLE_UUID'),
    ('ble_standard_uuid16', 'BLE_STANDARD_UUIDS_16', 'BLE_STD_UUID'),
    ('ble_manufacturer_id', 'BLE_MANUFACTURER_IDS', 'BLE_MFR'),
    ('ble_manufacturer_data', 'BLE_AD_BYTES_PATTERNS', 'BLE_AD_BYTES'),
]
RANK = {kind: i for i, (kind, _, _) in enumerate(GROUPS)}
RANK['legacy_ble_ad_bytes'] = len(GROUPS) - 1


def unique_object(pairs):
    obj = {}
    for key, value in pairs:
        if key in obj:
            raise ValueError(f'duplicate JSON key: {key}')
        obj[key] = value
    return obj


def load(path):
    return json.loads(Path(path).read_text(), object_pairs_hook=unique_object)


def rust_string(value):
    escaped = ''.join('\\' + c if c in ('\\', '"') else
                      f'\\u{{{ord(c):x}}}' if ord(c) < 32 else c for c in value)
    return '"' + escaped + '"'


def fingerprint(pack, kind):
    """Metadata edits do not alter numeric interpretation; behavior/IDs do."""
    if kind == 'pack':
        data = [{k: s[k] for k in ('id', 'index', 'enabled', 'match', 'label') if k in s}
                for s in pack['signatures']]
    else:
        data = {'pack_id': pack['pack_id'], 'engine': pack['engine'],
                'rules': [{k: r[k] for k in ('id', 'index', 'name', 'detect', 'category', 'enabled', 'alert')}
                          for r in pack['rules']]}
    return hashlib.sha256(json.dumps(data, sort_keys=True, separators=(',', ':'),
                                    ensure_ascii=False).encode()).hexdigest()


def compile_expr(expr, indices, depth=0):
    if depth > 16:
        raise ValueError('expression nesting exceeds compiled profile')
    if 'sig' in expr:
        if expr['sig'] not in indices:
            raise ValueError('unknown signature reference: ' + expr['sig'])
        return [('sig', indices[expr['sig']])]
    op = 'anyOf' if 'anyOf' in expr else 'allOf'
    nodes = []
    for child in expr[op]:
        nodes.extend(compile_expr(child, indices, depth + 1))
    nodes.append((op, len(expr[op])))
    return nodes


def validate(pack, revisions=None):
    import jsonschema
    jsonschema.Draft202012Validator(load(ROOT / 'schemas/signatures.v2.schema.json'),
                                   format_checker=jsonschema.FormatChecker()).validate(pack)
    indices = {}
    previous_rank = -1
    for i, s in enumerate(pack['signatures']):
        if s['index'] != i or s['id'] in indices:
            raise ValueError('signature indices must be contiguous and IDs unique')
        indices[s['id']] = i
        m = s['match']
        rank = RANK[m['kind']]
        if rank < previous_rank:
            raise ValueError('signature groups must follow the compiled profile order')
        previous_rank = rank
        if m['kind'] == 'ble_manufacturer_id' and m['value'] == 0:
            raise ValueError('company ID zero primitive needs embedded parser migration (#35)')
        if 'value' in m and isinstance(m['value'], str) and len(m['value'].encode()) > 32:
            raise ValueError('text predicate exceeds 32 UTF-8 bytes')
        if m['kind'].endswith('_contains'):
            if not m['value'].isascii() or m['value'] != m['value'].lower():
                raise ValueError('contains predicates require lowercase ASCII')
        if m['kind'] in ('mac_oui', 'wifi_ssid_pattern', 'ble_manufacturer_data', 'legacy_ble_ad_bytes') and 'label' not in s:
            raise ValueError('this predicate requires a label')
        if m['kind'] == 'wifi_ssid_pattern' and len(m['prefix'].encode()) + m['suffix_length'] > 32:
            raise ValueError('SSID pattern exceeds 32 bytes')
        if m['kind'] == 'ble_manufacturer_data' and len(m['bytes']) > m['exact_length']:
            raise ValueError('payload pattern exceeds exact field length')
        # Current primitives use first-value lookup; aliases would lose IDs.
        for prior in pack['signatures'][:i]:
            if prior['match'] == m:
                raise ValueError('duplicate primitive predicate')
    rule_ids = set()
    compiled = []
    for i, rule in enumerate(pack['rules']):
        if rule['index'] != i or rule['id'] in rule_ids:
            raise ValueError('rule indices must be contiguous and IDs unique')
        rule_ids.add(rule['id'])
        if len(rule['name'].encode()) > 32:
            raise ValueError('rule name exceeds 32 UTF-8 bytes')
        if rule['alert'] and not rule['enabled']:
            raise ValueError('disabled rule cannot be alert-enabled')
        nodes = compile_expr(rule['detect'], indices)
        stack = []
        for op, value in nodes:
            if op == 'sig':
                stack.append(({value}, 1))
            else:
                children = stack[-value:]
                del stack[-value:]
                leaves = set().union(*(c[0] for c in children))
                proof_size = sum(c[1] for c in children) if op == 'allOf' else max(c[1] for c in children)
                stack.append((leaves, proof_size))
            if len(stack) > 16:
                raise ValueError('expression evaluation stack exceeds 16 entries')
        leaves, proof_size = stack[0]
        if rule['enabled'] and any(not pack['signatures'][n]['enabled'] for n in leaves):
            raise ValueError('enabled rule references inactive signature')
        if rule['alert'] and proof_size > 4:
            raise ValueError('alert proof can exceed four evidence IDs')
        compiled.append(nodes)
    for item in pack['signatures'] + pack['rules']:
        if any(source not in pack['sources'] for source in item['review']['sources']):
            raise ValueError('unknown provenance source')
        if item['review']['status'] == 'device_validated':
            if not any(pack['sources'][source]['verification'] == 'device_capture'
                       for source in item['review']['sources']):
                raise ValueError('device validation requires a capture source')
    if sum(map(len, compiled)) > 65535:
        raise ValueError('expression pool exceeds u16 offsets')
    # Keep the worst compact companion inside the firmware's 512-byte line.
    max_rule = max(0, len(pack['rules']) - 1)
    worst = {'type':'detection', 'id':'FF:FF:FF:FF:FF:FF', 'scope':'wifi_address',
             'ts':4294967295, 'detection': {
                 'engine':pack['engine'], 'policy':'rules_and_watchlist',
                 'pack':pack['pack_id'], 'ruleset':pack['ruleset_id'],
                 'sig':list(range(248,256)),
                 'rules':[{'id':max_rule,'sig':[252,253,254,255],'more':False} for _ in range(4)],
                 'more':False, 'basis':'possibly_cached', 'alert':'suppressed'}}
    if len(json.dumps(worst, separators=(',', ':')).encode()) + 1 > 512:
        raise ValueError('pack/ruleset identity exceeds compact firmware evidence budget')
    if revisions is not None:
        for kind, field in [('pack', 'pack_id'), ('ruleset', 'ruleset_id')]:
            recorded = revisions[kind].get(pack[field])
            if recorded is None or recorded != fingerprint(pack, kind):
                raise ValueError(f'{field} revision is unregistered or changed; register a new revision')
    return compiled


def render(pack, compiled):
    lines = ['// @generated by scripts/compile-signatures.py; do not edit.',
             '// Canonical source: signatures/builtin.v2.json',
             '// Review provenance lives in the pack; not in firmware RAM.', '']
    groups = {kind: [] for kind, _, _ in GROUPS}
    for s in pack['signatures']:
        k = s['match']['kind']
        groups['ble_manufacturer_data' if k == 'legacy_ble_ad_bytes' else k].append(s)
    start = 0
    for kind, name, constant in GROUPS:
        entries = groups[kind]
        lines.append(f'pub const SIG_IDX_{constant}_START: SigIdx = {start};')
        start += len(entries)
        if kind == 'mac_oui':
            ty = '&[([u8; 3], &str)]'
        elif kind == 'wifi_ssid_pattern':
            ty = '&[SsidPattern]'
        elif kind in ('ble_service_uuid16','ble_standard_uuid16','ble_manufacturer_id'):
            ty = '&[u16]'
        elif kind == 'ble_manufacturer_data':
            ty = '&[BleAdBytesPattern]'
        else:
            ty = '&[&str]'
        lines.append(f'pub static {name}: {ty} = &[')
        for entry in entries:
            m = entry['match']
            label = rust_string(entry.get('label', ''))
            if kind == 'mac_oui':
                octets = ', '.join('0x' + n for n in m['oui'].split(':'))
                lines.append(f'    ([{octets}], {label}),')
            elif kind == 'wifi_ssid_pattern':
                suffix = {'hex':'HexChars','decimal':'DecimalDigits'}[m['suffix_kind']]
                lines.append('    SsidPattern { prefix: ' + rust_string(m['prefix']) +
                             f', suffix_len: {m["suffix_length"]}, suffix_kind: SuffixKind::{suffix}, description: {label} }},')
            elif kind == 'ble_manufacturer_data':
                enabled = str(entry['enabled']).lower()
                if m['kind'] == 'legacy_ble_ad_bytes':
                    values, offset, length = m['bytes'], 'None', 'None'
                else:
                    values = [m['company_id'] & 255, m['company_id'] >> 8] + m['bytes']
                    offset, length = 'Some(0)', f'Some({m["exact_length"] + 2})'
                data = ', '.join(f'0x{x:02X}' for x in values)
                lines.append(f'    BleAdBytesPattern {{ active: {enabled}, exact_manufacturer_len: {length}, bytes: &[{data}], offset: {offset}, description: {label} }},')
            elif ty == '&[u16]':
                lines.append(f'    0x{m["value"]:04X},')
            else:
                lines.append('    ' + rust_string(m['value']) + ',')
        lines.extend(['];', ''])
    lines.append(f'pub const SIG_COUNT: usize = {start};')
    lines.append('pub const SIGNATURE_PACK_ID: &str = ' + rust_string(pack['pack_id']) + ';')
    lines.append('static DEFAULT_RULE_NODES: &[ExprNode] = &[')
    for nodes in compiled:
        for op, value in nodes:
            lines.append(f'    ExprNode::Sig({value}),' if op == 'sig' else
                         f'    ExprNode::{"AnyOf" if op == "anyOf" else "AllOf"} {{ count: {value} }},')
    lines.extend(['];', '', 'static DEFAULT_RULES: &[Rule] = &['])
    start = 0
    for rule, nodes in zip(pack['rules'], compiled):
        lines.append('    Rule { name: ' + rust_string(rule['name']) +
                     f', expr_start: {start}, expr_len: {len(nodes)}, category: MatchCategory::{rule["category"].title()}, active: {str(rule["enabled"]).lower()}, alert: {str(rule["alert"]).lower()} }},')
        start += len(nodes)
    lines.extend(['];', '', 'pub static DEFAULT_RULE_DB: RuleDb = RuleDb {',
                  '    id: ' + rust_string(pack['ruleset_id']) + ',',
                  '    nodes: DEFAULT_RULE_NODES,', '    rules: DEFAULT_RULES,', '};', ''])
    return '\n'.join(lines)



def check_revision_history(current, previous):
    for kind in ('pack', 'ruleset'):
        for revision, digest in previous[kind].items():
            if current[kind].get(revision) != digest:
                raise ValueError(f'published {kind} revision {revision} was changed or removed')


def previous_revisions(ref):
    if not re.fullmatch(r'[0-9a-fA-F]{40}', ref):
        raise ValueError('base ref must be a full commit SHA')
    subprocess.run(['git', 'cat-file', '-e', ref], cwd=ROOT, check=True, capture_output=True)
    exists = subprocess.run(['git', 'ls-tree', '--name-only', ref, '--', 'signatures/revisions.json'],
                            cwd=ROOT, check=True, capture_output=True, text=True)
    if not exists.stdout.strip():
        return {'pack':{}, 'ruleset':{}}
    result = subprocess.run(['git', 'show', ref + ':signatures/revisions.json'],
                            cwd=ROOT, check=True, capture_output=True, text=True)
    return json.loads(result.stdout, object_pairs_hook=unique_object)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='Fail if generated tables are stale')
    parser.add_argument('--base-ref', help='Reject changes to revision identities published at this full commit SHA')
    args = parser.parse_args()
    pack = load(ROOT / 'signatures/builtin.v2.json')
    revisions = load(ROOT / 'signatures/revisions.json')
    if args.base_ref:
        check_revision_history(revisions, previous_revisions(args.base_ref))
    compiled = validate(pack, revisions)
    output = render(pack, compiled)
    path = ROOT / 'src/generated_signatures.rs'
    if args.check:
        if not path.exists() or path.read_text() != output:
            parser.exit(1, 'Generated signatures are stale; run scripts/compile-signatures.py\n')
    else:
        path.write_text(output)
    print(f'Canonical signatures: {len(pack["signatures"])} predicates, {len(compiled)} rules; IDs and revisions verified.')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(f'Signature compilation failed: {error}')
