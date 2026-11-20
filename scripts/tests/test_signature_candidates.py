"""Offline policy checks for the research pack, not a runtime signature loader."""
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]


def load(path):
    return json.loads((ROOT / path).read_text())


def matches(signature, observation):
    kind = signature['type']
    if kind == 'ble_manufacturer_id':
        return signature['company_id'] in observation.get('company_ids', [])
    if kind == 'ble_service_uuid':
        # Fixtures use canonical lowercase UUIDs; no lossy short-UUID extraction.
        return signature['uuid'] in observation.get('service_uuids', [])
    if kind == 'ble_name' and signature['match'] == 'regex':
        flags = 0 if signature.get('case_sensitive', True) else re.IGNORECASE
        return re.search(signature['value'], observation.get('name', ''), flags) is not None
    raise AssertionError(f'Add explicit fixture semantics for {kind}')


def evaluate(expression, hits):
    if 'sig' in expression:
        return expression['sig'] in hits
    if 'allOf' in expression:
        return all(evaluate(child, hits) for child in expression['allOf'])
    if 'anyOf' in expression:
        return any(evaluate(child, hits) for child in expression['anyOf'])
    if 'not' in expression:
        return not evaluate(expression['not'], hits)
    raise AssertionError('Unknown expression')


def references(expression):
    if 'sig' in expression:
        return {expression['sig']}
    if 'not' in expression:
        return references(expression['not'])
    children = expression.get('allOf', expression.get('anyOf', []))
    return set().union(*(references(child) for child in children))


class SignatureCandidateTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.pack = load('schemas/examples/research-devices.sigs.json')
        cls.provenance = load('signatures/research-provenance.json')
        cls.fixtures = load('signatures/fixtures/research-devices.json')['cases']

    def test_every_signature_has_sources_and_explicit_unvalidated_status(self):
        ids = [s['id'] for s in self.pack['signatures']]
        self.assertEqual(len(ids), len(set(ids)))
        self.assertEqual(set(ids), set(self.provenance['signatures']))
        self.assertIs(self.provenance['default_enabled'], False)
        for evidence in self.provenance['signatures'].values():
            self.assertEqual(evidence['status'], 'research_candidate')
            self.assertIs(evidence['field_validated'], False)
            self.assertTrue(evidence['sources'])
            for source in evidence['sources']:
                self.assertIn(source, self.provenance['sources'])
                self.assertTrue(self.provenance['sources'][source]['url'].startswith('https://'))

    def test_rule_references_are_valid_and_ids_unique(self):
        signature_ids = {s['id'] for s in self.pack['signatures']}
        rule_ids = [r['id'] for r in self.pack['rules']]
        self.assertEqual(len(rule_ids), len(set(rule_ids)))
        referenced = set()
        for rule in self.pack['rules']:
            refs = references(rule['detect'])
            self.assertTrue(refs)
            self.assertLessEqual(refs, signature_ids)
            referenced.update(refs)
            self.assertIn('research', rule['tags'])
        self.assertEqual(referenced, signature_ids)

    def test_synthetic_observations_and_false_positive_controls(self):
        exercised_signatures, exercised_rules = set(), set()
        for fixture in self.fixtures:
            with self.subTest(fixture=fixture['id']):
                hits = {s['id'] for s in self.pack['signatures']
                        if matches(s, fixture['observation'])}
                result = {r['id'] for r in self.pack['rules'] if evaluate(r['detect'], hits)}
                self.assertEqual(result, set(fixture['expected_rules']))
                exercised_signatures.update(hits)
                exercised_rules.update(result)
        self.assertEqual(exercised_signatures, {s['id'] for s in self.pack['signatures']})
        self.assertEqual(exercised_rules, {r['id'] for r in self.pack['rules']})
