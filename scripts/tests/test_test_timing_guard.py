"""Adversarial source snippets are detector input, never executed fixtures."""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'check-test-timing.py'
spec = importlib.util.spec_from_file_location('timing_guard', SCRIPT)
guard = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = guard
spec.loader.exec_module(guard)


class TimingGuardTests(unittest.TestCase):
    def found(self, source, path='src/example.rs'):
        findings = guard.scan(path, source)
        self.assertTrue(findings, source)
        return findings

    def test_visibility_nested_and_individual_test_scopes(self):
        for source in [
            '#[cfg(test)] pub(crate) mod tests { fn helper(){ std::thread::sleep(d); } }',
            '#[cfg(all(test, unix))] pub mod checks { mod nested { fn f(){ Instant::now(); } } }',
            '#[tokio::test(start_paused = true)] async fn f(){ tokio::time::sleep(d).await; }',
            '#[cfg(test)] fn helper(){ std::time::SystemTime::now(); }',
        ]:
            self.found(source)

    def test_global_and_transitive_aliases_bind_declaration(self):
        source = 'use std::thread::sleep as nap; use nap as snooze; #[test] fn example(){ snooze(d); }'
        first = self.found(source)
        self.assertTrue(any(item.rule == 'timing-alias' for item in first))
        second = self.found(source.replace('std::thread::sleep', 'tokio::time::sleep'))
        self.assertNotEqual({item.key for item in first}, {item.key for item in second})
        self.found('const LATER: fn() = std::thread::sleep; #[test] fn f(){ LATER(); }')

    def test_production_scope_and_comments_are_not_blanket_test_scopes(self):
        self.assertEqual(guard.scan('src/runtime.rs', 'fn runtime(){ std::thread::sleep(d); }'), [])
        self.assertEqual(guard.scan('src/runtime.rs', '// #[test] fn f(){ sleep(d); }\nfn runtime(){}'), [])
        self.assertEqual(guard.scan('tests/clean.rs', 'fn f(){ /* sleep(d); */ consume(42); }'), [])

    def test_embedded_raw_scripts_and_multiline_calls(self):
        self.found('#[test] fn f(){ let script = r###"#!/bin/sh\nsleep .01\n"###; }')
        self.found('func testFixture(){ let script = #"""\n/bin/sleep .01\n"""# }', 'apps/macos/Tests/Fixture.swift')
        self.found('def test_fixture():\n    script = """import time\ntime.sleep(0.1)"""', 'scripts/tests/test_fixture.py')
        self.found('#[test] fn f(){ tokio::time::timeout\n (limit, work()); }')

    def test_escaped_identifiers_and_continuations_keep_owner(self):
        source = r'#[test] fn first(){ let s="set\u0054imeout(f, 2)"; }' + '\n#[test] fn second(){ sleep(d); }'
        findings = self.found(source)
        self.assertTrue(any(item.scope == 'first' and item.rule == 'clock-reference' for item in findings))
        self.assertTrue(any(item.scope == 'second' and item.rule == 'delay-or-deadline' for item in findings))
        self.found('sl\\\neep .01', 'tests/fixture.sh')

    def test_reflection_and_javascript_alias_references(self):
        self.found('getattr(time, "sleep")(0.1)', 'scripts/tests/test_fixture.py')
        self.found('const {setTimeout: later} = globalThis; later(callback, 2);', 'website/tests/timing.test.mjs')
        self.found('from time import sleep as later\nlater(.1)', 'scripts/tests/test_fixture.py')

    def test_ordinary_and_nanosecond_clock_apis_and_alias_imports(self):
        for source in [
            'time.time()', 'time.time_ns()', 'time.monotonic_ns()',
            'time.perf_counter_ns()', 'loop.time()',
            'asyncio.get_running_loop().time()',
            'from time import time as wall\nwall()',
            'from time import monotonic_ns as tick\ntick()',
            'getattr(time, "time_ns")()',
            'datetime.datetime.today()',
        ]:
            self.found(source, 'scripts/tests/test_clock.py')
        for source in ['Date.now', 'performance.timeOrigin', 'process.hrtime.bigint()']:
            self.found(source, 'website/tests/clock.test.mjs')
        for source in ['ProcessInfo.processInfo.systemUptime', 'CACurrentMediaTime()']:
            self.found(source, 'apps/macos/Tests/ClockTests.swift')

    def test_fingerprint_detects_changed_scope_and_duplicate_call(self):
        first = self.found('#[test] fn f(){ sleep(d); assert!(ready); }')
        changed = self.found('#[test] fn f(){ sleep(d); assert!(true); }')
        self.assertNotEqual(first[0].key, changed[0].key)
        duplicate = self.found('#[test] fn f(){ sleep(d); sleep(d); }')
        self.assertEqual(len({item.key for item in duplicate}), len(duplicate))
        self.assertGreater(len(duplicate), len(first))

    def test_external_cfg_module_resolution_and_missing_target_refusal(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'src').mkdir()
            (root / 'src/clock_cases.rs').write_text('fn hidden(){ std::thread::sleep(d); }')
            source = '#[cfg(test)] pub(crate) mod clock_cases;'
            targets, errors = guard.external_test_modules('src/lib.rs', source, root, helper=False)
            self.assertEqual(targets, {'src/clock_cases.rs'})
            self.assertEqual(errors, [])
            self.assertTrue(guard.scan('src/clock_cases.rs', (root / 'src/clock_cases.rs').read_text(), helper=True))
            (root / 'src/clock_cases.rs').unlink()
            self.assertTrue(guard.external_test_modules('src/lib.rs', source, root, helper=False)[1])

    def test_external_nested_module_and_literal_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'src/tests').mkdir(parents=True)
            (root / 'src/tests/helper.rs').write_text('fn helper(){}')
            source = '#[cfg(test)] mod tests { pub(crate) mod helper; }'
            self.assertEqual(guard.external_test_modules('src/lib.rs', source, root, helper=False)[0], {'src/tests/helper.rs'})
            for source in [
                '#[cfg(test)] #[path="tests/helper.rs"] mod cases;',
                '#[path="tests/helper.rs"] #[cfg(test)] pub(crate) mod cases;',
            ]:
                self.assertEqual(guard.external_test_modules('src/lib.rs', source, root, helper=False), ({'src/tests/helper.rs'}, []))

    def test_duration_inputs_alone_are_not_clock_evidence_but_client_seams_are(self):
        self.assertEqual(guard.scan('tests/data.rs', 'fn data(){ Duration::from_secs(5); }'), [])
        self.found('#[test] fn f(){ Client::connect(path, duration); }')
        self.found('#[test] fn f(){ Client::handshake(stream, duration); }')

    def test_policy_cannot_use_path_wildcards_or_unreviewed_safe_classification(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'policy.json'
            for policy in [
                {'version': 1, 'helpers': {'src/*': 'all helpers'}, 'dispositions': {}},
                {'version': 1, 'helpers': {}, 'dispositions': {'a'*64: {'category': 'outer-hang-guard', 'reason': 'bounded', 'tracking': '#329'}}},
            ]:
                path.write_text(json.dumps(policy))
                with self.assertRaises(ValueError): guard.read_policy(path)

    def test_duplicate_policy_keys_fail_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'policy.json'
            path.write_text('{"version":1,"helpers":{},"helpers":{"src/runtime.rs":"override"}}')
            with self.assertRaisesRegex(ValueError, 'duplicate policy key'):
                guard.read_policy(path)

    def test_unresolved_baseline_remains_visible_and_new_sites_fail(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            (root / 'tests').mkdir()
            source = 'sleep(.1)'
            (root / 'tests/test_fixture.py').write_text(source)
            subprocess.run(['git', 'add', '.'], cwd=root, check=True)
            finding = guard.scan('tests/test_fixture.py', source)[0]
            policy = {'version': 1, 'helpers': {}, 'dispositions': {finding.key: {'category': 'unresolved', 'reason': 'migration pending', 'tracking': '#329', 'path': finding.path, 'scope': finding.scope, 'rule': finding.rule, 'context_sha256': finding.context_sha256}}}
            report = guard.report(root, policy)
            self.assertEqual(report['new_unclassified'], 0)
            self.assertEqual(report['unresolved_existing'], 1)
            policy['dispositions'][finding.key]['path'] = 'other.py'
            self.assertTrue(guard.report(root, policy)['scan_errors'])
            policy['dispositions'][finding.key]['path'] = finding.path
            (root / 'tests/test_fixture.py').write_text(source + '\nsleep(.2)')
            self.assertGreater(guard.report(root, policy)['new_unclassified'], 0)

    def test_frozen_migration_baseline_and_closure_exit_codes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            (root / 'tests').mkdir()
            fixture = root / 'tests/test_fixture.py'
            fixture.write_text('time.time()')
            subprocess.run(['git', 'add', '.'], cwd=root, check=True)
            keys = sorted(item.key for item in guard.scan('tests/test_fixture.py', fixture.read_text()))
            baseline = {
                'source_commit': 'a'*40, 'source_tree': 'b'*40,
                'generator_sha256': 'c'*64,
                'inventory_sha256': hashlib.sha256(json.dumps(keys, separators=(',', ':')).encode()).hexdigest(),
                'fingerprints': keys, 'count': len(keys), 'tracking': '#329',
                'note': 'Unresolved migration; this is not acceptance.',
            }
            policy = {'version': 1, 'helpers': {}, 'dispositions': {}, 'migration_baseline': baseline}
            path = root / 'policy.json'
            path.write_text(json.dumps(policy))
            args = [sys.executable, str(SCRIPT), '--root', str(root), '--policy', str(path)]
            ordinary = subprocess.run(args, capture_output=True, text=True, check=False)
            self.assertEqual(ordinary.returncode, 0, ordinary.stderr)
            self.assertIn('NOT established', ordinary.stderr)
            closure = subprocess.run(args + ['--require-resolved'], capture_output=True, check=False)
            self.assertEqual(closure.returncode, 1)
            fixture.write_text('time.time_ns()')
            changed = subprocess.run(args, capture_output=True, text=True, check=False)
            self.assertEqual(changed.returncode, 1)
            self.assertEqual(json.loads(changed.stdout)['stale_migration_fingerprints'], keys)
            baseline['fingerprints'] = sorted(keys + ['d'*64])
            path.write_text(json.dumps(policy))
            with self.assertRaisesRegex(ValueError, 'inventory hash'):
                guard.read_policy(path)

    def test_unterminated_scope_fails_closed(self):
        with self.assertRaises(ValueError): guard.scan('src/lib.rs', '#[test] fn f(){ let s=r#"sleep(1)')


if __name__ == '__main__':
    unittest.main()
