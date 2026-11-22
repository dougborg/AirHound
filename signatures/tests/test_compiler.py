"""Behavioral and rejection tests for canonical-to-firmware compilation."""
import copy
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('compiler', ROOT / 'scripts/compile-signatures.py')
compiler = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compiler)


class CompilerTests(unittest.TestCase):
    def setUp(self):
        self.pack = compiler.load(ROOT / 'signatures/builtin.v2.json')
        self.revisions = compiler.load(ROOT / 'signatures/revisions.json')

    def rejects(self, mutate):
        mutate(self.pack)
        with self.assertRaises((ValueError, __import__('jsonschema').ValidationError)):
            compiler.validate(self.pack)

    def test_current_catalog_preserves_wire_id_slots_and_rule_proofs(self):
        nodes = compiler.validate(self.pack, self.revisions)
        self.assertEqual(len(self.pack['signatures']), 144)
        self.assertEqual([r['index'] for r in self.pack['rules']], list(range(7)))
        self.assertEqual(nodes[0], [('sig',0),('sig',117),('allOf',2),
                                   ('sig',0),('sig',119),('allOf',2),
                                   ('sig',140),('sig',124),('allOf',2),
                                   ('sig',140),('sig',126),('allOf',2),('anyOf',4)])
        self.assertEqual(nodes[-1], [('sig',136)])
        self.assertEqual([s['index'] for s in self.pack['signatures'] if not s['enabled']], [142,143])

    def test_generated_tables_are_exactly_current(self):
        expected = compiler.render(self.pack, compiler.validate(self.pack, self.revisions))
        self.assertEqual((ROOT / 'src/generated_signatures.rs').read_text(), expected)

    def test_contextual_bytes_compile_without_offset_reinterpretation(self):
        text = compiler.render(self.pack, compiler.validate(self.pack))
        self.assertIn('exact_manufacturer_len: Some(29), bytes: &[0x4C, 0x00, 0x12, 0x19], offset: Some(0)', text)
        self.assertIn('active: false, exact_manufacturer_len: None, bytes: &[0x80, 0x30], offset: None', text)

    def test_duplicate_keys_and_ids_and_wrong_indices_fail(self):
        with self.assertRaisesRegex(ValueError, 'duplicate JSON key'):
            json.loads('{"version":2,"version":1}', object_pairs_hook=compiler.unique_object)
        for field, value in [('id',self.pack['signatures'][0]['id']),('index',0)]:
            pack = copy.deepcopy(self.pack)
            pack['signatures'][1][field] = value
            with self.assertRaises(ValueError):
                compiler.validate(pack)

    def test_missing_refs_disabled_rules_and_inactive_predicates_fail(self):
        for mutation in [
            lambda p: p['rules'][0].update(detect={'sig':'missing'}),
            lambda p: p['rules'][0].update(enabled=False),
            lambda p: p['rules'][3].update(enabled=True),
        ]:
            self.setUp()
            self.rejects(mutation)

    def test_unknown_fields_unsupported_uuid_and_masks_fail_closed(self):
        for mutation in [
            lambda p: p.update(unknown=True),
            lambda p: p['signatures'][0]['match'].update(typo=1),
            lambda p: p['signatures'][131]['match'].update(value='12345678-1234-1234-1234-123456789012'),
            lambda p: p['signatures'][141]['match'].update(mask=[255,255]),
            lambda p: p['signatures'][141]['match'].update(offset=1),
            lambda p: p['signatures'][142].update(enabled=True),
            lambda p: p['rules'][0].update(detect={'not':{'sig':p['signatures'][0]['id']}}),
        ]:
            self.setUp()
            self.rejects(mutation)

    def test_company_zero_is_a_valid_contextual_manufacturer_id(self):
        self.pack['signatures'][140]['match']['value'] = 0
        text = compiler.render(self.pack, compiler.validate(self.pack))
        self.assertIn('pub static BLE_MANUFACTURER_IDS: &[u16] = &[\n    0x0000,\n];', text)

    def test_bounds_context_and_provenance_are_enforced(self):
        for mutation in [
            lambda p: p['signatures'][141]['match'].update(exact_length=1),
            lambda p: p['signatures'][117]['match'].update(suffix_length=32),
            lambda p: p['signatures'][119]['match'].update(value='é'*20),
            lambda p: p['signatures'][124]['match'].update(value='FLOCK'),
            lambda p: p['signatures'][0]['review'].update(sources=['missing']),
            lambda p: p['signatures'][0]['review'].update(status='device_validated'),
            lambda p: p.update(pack_id='x'*64, ruleset_id='y'*64),
        ]:
            self.setUp()
            self.rejects(mutation)

    def test_expression_stack_and_alert_proof_bounds(self):
        leaves = [{'sig':s['id']} for s in self.pack['signatures'][:5]]
        self.rejects(lambda p: p['rules'][0].update(detect={'allOf':leaves}))
        self.setUp()
        # 15 saved children plus a nested two-leaf expression exceeds stack 16.
        leaves = [{'sig':s['id']} for s in self.pack['signatures'][:15]]
        leaves.append({'anyOf':[{'sig':self.pack['signatures'][0]['id']},
                                {'sig':self.pack['signatures'][1]['id']}]})
        self.rejects(lambda p: p['rules'][1].update(detect={'anyOf':leaves}))

    def test_revision_reuse_fails_but_review_notes_do_not_change_meaning(self):
        self.pack['signatures'][0]['review']['limitations'] = 'Reviewed documentation only.'
        compiler.validate(self.pack, self.revisions)
        self.pack['rules'][1]['alert'] = True
        with self.assertRaisesRegex(ValueError, 'ruleset_id revision'):
            compiler.validate(self.pack, self.revisions)
        self.setUp()
        self.pack['signatures'][0]['label'] += ' changed'
        with self.assertRaisesRegex(ValueError, 'pack_id revision'):
            compiler.validate(self.pack, self.revisions)

    def test_published_revision_history_cannot_be_rewritten_or_removed(self):
        compiler.check_revision_history(self.revisions, copy.deepcopy(self.revisions))
        old = copy.deepcopy(self.revisions)
        self.revisions['pack']['builtin-2'] = 'replacement'
        with self.assertRaisesRegex(ValueError, 'changed or removed'):
            compiler.check_revision_history(self.revisions, old)
        del self.revisions['pack']['builtin-2']
        with self.assertRaisesRegex(ValueError, 'changed or removed'):
            compiler.check_revision_history(self.revisions, old)

    def test_string_literals_do_not_inject_rust(self):
        self.assertEqual(compiler.rust_string('a"b\\c\n'), '"a\\"b\\\\c\\u{a}"')


if __name__ == '__main__':
    unittest.main()
