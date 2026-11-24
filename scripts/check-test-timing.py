#!/usr/bin/env python3
"""Report test-owned timing sites; require exact, reviewed dispositions.

This is a lexical regression guard, not a proof of timing independence. It scans
embedded strings as well as code, and preserves unresolved migration findings.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from dataclasses import asdict, dataclass
from pathlib import Path

EXTENSIONS = {'.rs', '.swift', '.py', '.js', '.mjs', '.cjs', '.ts', '.tsx', '.sh'}
CATEGORIES = {'virtual-time', 'causal-event', 'outer-hang-guard', 'fixed-input',
              'benchmark-or-physical', 'production-scope', 'detector-input', 'unresolved'}
RULES = {
    'delay-or-deadline': re.compile(r'\b(?:sleep|sleep_until|sleep_for|timeout|timeout_at|recv_timeout|wait_timeout|wait_for|wait_until|nanosleep|usleep|yield_now)\s*(?=\(|[\d.]|as\b)'),
    'clock-reference': re.compile(r'\b(?:Instant|SystemTime|ContinuousClock|SuspendingClock|DispatchTime|monotonic|perf_counter|setTimeout|setInterval|waitForTimeout|advanceTimersByTime|useFakeTimers|requestAnimationFrame)\b'),
    'clock-read': re.compile(r'(?:\.|::)\s*(?:now|elapsed|utcnow|hrtime|today|systemUptime|timeOrigin)\b|(?:\.|::)\s*time\b(?=\s*\()|\b(?:new\s+)?Date\s*\('),
    'clock-function': re.compile(r'\b(?:time_ns|monotonic_ns|perf_counter_ns|process_time(?:_ns)?|thread_time(?:_ns)?|clock_gettime(?:_ns)?|gettimeofday|CFAbsoluteTimeGetCurrent|CACurrentMediaTime|mach_absolute_time)\b|\btime\s*\('),
    'delay-import': re.compile(r'\b(?:sleep|timeout|time|time_ns|monotonic|monotonic_ns|perf_counter|perf_counter_ns)\s*(?:as\s+\w+|[,}])'),
    'indirect-delay': re.compile(r'\bgetattr\s*\([^,\n]+,\s*[\'\"](?:sleep|timeout|time|time_ns|monotonic|monotonic_ns|perf_counter|perf_counter_ns)[\'\"]'),
    'client-deadline-seam': re.compile(r'\b(?:Client|BackendClient)\s*::\s*(?:connect|handshake)\s*\('),
    'shell-delay': re.compile(r'(?:^|[;\n]|/bin/)\s*(?:command\s+)?sleep\s+[^\s;]+', re.MULTILINE),
}


@dataclass(frozen=True)
class Finding:
    key: str
    path: str
    line: int
    scope: str
    rule: str
    excerpt: str
    context_sha256: str


def lexical_views(source: str, language: str) -> tuple[str, str]:
    """Return brace-safe code and comment-free text retaining embedded strings."""
    code, content = list(source), list(source)
    n, i = len(source), 0
    def erase(target: list[str], start: int, end: int) -> None:
        target[start:end] = ['\n' if c == '\n' else ' ' for c in source[start:end]]
    while i < n:
        start = i
        if source.startswith('//', i) and language not in {'.py', '.sh'} or source[i] == '#' and language in {'.py', '.sh'}:
            i = source.find('\n', i)
            if i < 0: i = n
            erase(code, start, i); erase(content, start, i); continue
        if source.startswith('/*', i) and language not in {'.py', '.sh'}:
            depth, i = 1, i + 2
            while i < n and depth:
                if source.startswith('/*', i): depth += 1; i += 2
                elif source.startswith('*/', i): depth -= 1; i += 2
                else: i += 1
            if depth: raise ValueError('unterminated block comment')
            erase(code, start, i); erase(content, start, i); continue
        raw = re.match(r'(?:br|rb|r)(#{0,255})"', source[i:]) if language == '.rs' else None
        swift = re.match(r'(#+)("""|")', source[i:]) if language == '.swift' else None
        if raw or swift:
            match = raw or swift
            hashes = match.group(1)
            quote = '"' if raw else match.group(2)
            end = source.find(quote + hashes, i + match.end())
            if end < 0: raise ValueError('unterminated raw string')
            i = end + len(quote) + len(hashes)
            erase(code, start, i); continue
        quote = source[i]
        if quote in '\'"`':
            # Rust lifetimes are identifiers, not strings.
            if language == '.rs' and quote == "'" and not re.match(r"'(?:[^'\\\n]|\\(?:.|x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]+\}))'", source[i:]):
                i += 1; continue
            delimiter = quote * 3 if source.startswith(quote * 3, i) and quote != '`' else quote
            i += len(delimiter)
            while i < n:
                if source[i] == '\\': i += 2
                elif source.startswith(delimiter, i): i += len(delimiter); break
                else: i += 1
            else: raise ValueError('unterminated string')
            erase(code, start, i); continue
        i += 1
    return ''.join(code), ''.join(content)


def brace_pairs(code: str) -> dict[int, int]:
    stack, pairs = [], {}
    for index, char in enumerate(code):
        if char == '{': stack.append(index)
        elif char == '}':
            if not stack: raise ValueError('unmatched closing brace')
            pairs[stack.pop()] = index + 1
    if stack: raise ValueError('unclosed brace')
    return pairs


def test_path(path: str) -> bool:
    p = Path(path)
    parts = {part.lower() for part in p.parts}
    name = p.name.lower()
    return bool(parts & {'tests', 'test', 'testsupport', 'benches', 'benchmarks'} or
                re.search(r'(^test[-_]|[._-]tests?[._-]|tests?\.swift$)', name))


def rust_regions(code: str, pairs: dict[int, int]) -> list[tuple[int, int, str]]:
    regions = []
    for attribute in re.finditer(r'#\s*\[([^\]]*)\]', code, re.DOTALL):
        body = attribute.group(1)
        is_test = bool(re.match(r'\s*(?:[\w]+::)*test\b', body) or
                       re.match(r'\s*cfg\s*\(', body) and re.search(r'\btest\b', body))
        if not is_test: continue
        start = attribute.end()
        brace, semicolon = code.find('{', start), code.find(';', start)
        if brace < 0 or 0 <= semicolon < brace:
            # External cfg(test) modules require separate scope registration.
            end = semicolon + 1 if semicolon >= 0 else len(code)
            regions.append((attribute.start(), end, 'rust-test-item'))
        else:
            end = pairs.get(brace)
            if end is None: raise ValueError('test item has unmatched body')
            header = code[start:brace]
            name = re.search(r'\b(?:mod|fn)\s+([A-Za-z_]\w*)', header)
            regions.append((attribute.start(), end, name.group(1) if name else 'rust-test-item'))
    return regions


TIMING_NAMES = {'sleep', 'sleep_until', 'sleep_for', 'timeout', 'timeout_at',
                'recv_timeout', 'wait_timeout', 'monotonic', 'perf_counter', 'yield_now',
                'Instant', 'SystemTime', 'now', 'elapsed', 'time', 'time_ns', 'monotonic_ns', 'perf_counter_ns'}


def rust_aliases(code: str) -> dict[str, str]:
    bindings = {}
    clauses = [match.group() for match in re.finditer(r'\buse\s+[^;]+;', code)]
    # Resolve chains without depending on declaration order. The finite set of
    # aliases only grows; this is static graph traversal, not scheduler polling.
    changed = True
    while changed:
        changed = False
        for clause in clauses:
            for name, alias in re.findall(r'\b([A-Za-z_]\w*)\s+as\s+([A-Za-z_]\w*)', clause):
                if alias not in bindings and (name in TIMING_NAMES or name in bindings):
                    bindings[alias] = bindings.get(name, '') + clause
                    changed = True
    for match in re.finditer(r'\b(?:let|const|static)\s+(?:mut\s+)?([A-Za-z_]\w*)[^;=]*=\s*([A-Za-z_][\w:]*)\s*;', code):
        name = match.group(2).rsplit('::', 1)[-1]
        if name in TIMING_NAMES or name in bindings:
            bindings[match.group(1)] = bindings.get(name, '') + match.group()
    return bindings


def external_test_modules(path: str, source: str, root: Path, *, helper: bool) -> tuple[set[str], list[str]]:
    code, _ = lexical_views(source, '.rs')
    pairs = brace_pairs(code)
    regions = [(0, len(code), 'fixture-file')] if helper or test_path(path) else rust_regions(code, pairs)
    containers = []
    for match in re.finditer(r'\bmod\s+([A-Za-z_]\w*)\s*\{', code):
        if match.end() - 1 in pairs:
            containers.append((match.start(), pairs[match.end() - 1], match.group(1)))
    targets, errors, visited = set(), [], set()
    for start, end, _ in regions:
        for match in re.finditer(r'\bmod\s+([A-Za-z_]\w*)\s*;', code[start:end]):
            position = start + match.start()
            if position in visited: continue
            visited.add(position)
            parents = [name for begin, finish, name in sorted(containers) if begin < position < finish]
            file = Path(path)
            bases = [file.parent.joinpath(*parents)]
            if file.stem not in {'lib', 'main', 'mod'}:
                bases.append(file.parent.joinpath(file.stem, *parents))
            # Adjacent attributes before this declaration; comments/strings do
            # not determine scope, but a literal path attribute determines file.
            prefix_start = max(code.rfind(';', 0, position), code.rfind('}', 0, position), code.rfind('{', 0, position), 0)
            prefix = source[prefix_start:position]
            attributes = re.findall(r'#\s*\[\s*path\s*=\s*"([^"\n]+)"\s*\]', prefix)
            candidates = [base / attributes[-1] for base in bases] if attributes else [
                candidate for base in bases for candidate in (base / (match.group(1) + '.rs'), base / match.group(1) / 'mod.rs')]
            existing = set()
            for candidate in candidates:
                absolute = (root / candidate).resolve()
                try: relative = absolute.relative_to(root.resolve()).as_posix()
                except ValueError: continue
                if absolute.is_file(): existing.add(relative)
            if len(existing) != 1:
                errors.append(f'external test module {match.group(1)} has {len(existing)} resolvable tracked-path candidates')
            else: targets.update(existing)
    return targets, errors


def normalized(text: str) -> tuple[str, list[int]]:
    # Preserve source offsets while decoding common identifier escapes and
    # joining continuations; owning-scope hashes must bind the real source.
    output, offsets = [], []
    token = re.compile(r'\\(?:\r?\n|u([0-9a-fA-F]{4})|x([0-9a-fA-F]{2}))')
    cursor = 0
    for match in token.finditer(text):
        output.extend(text[cursor:match.start()]); offsets.extend(range(cursor, match.start()))
        if match.group(1) or match.group(2):
            output.append(chr(int(match.group(1) or match.group(2), 16)))
            offsets.append(match.start())
        cursor = match.end()
    output.extend(text[cursor:]); offsets.extend(range(cursor, len(text)))
    return ''.join(output), offsets


def scan(path: str, source: str, *, helper: bool = False) -> list[Finding]:
    language = Path(path).suffix
    code, content = lexical_views(source, language)
    pairs = brace_pairs(code) if language in {'.rs', '.swift'} else {}
    regions = [(0, len(source), 'fixture-file')] if test_path(path) or helper else (
        rust_regions(code, pairs) if language == '.rs' else [])
    findings, seen = [], set()
    aliases = rust_aliases(code) if language == '.rs' else {}
    rules = dict(RULES)
    if aliases:
        rules['timing-alias'] = re.compile(r'\b(' + '|'.join(map(re.escape, sorted(aliases))) + r')\s*(?=\(|::)')
    # An owning function fingerprint keeps unrelated function edits from
    # invalidating another reviewed disposition; embedded literals stay included.
    functions = []
    for match in re.finditer(r'\b(?:fn|func)\s+([A-Za-z_]\w*)[^;{}]*\{', code):
        brace = match.end() - 1
        if brace in pairs: functions.append((match.start(), pairs[brace], match.group(1)))
    for start, end, region_name in regions:
        text, offsets = normalized(content[start:end])
        # Fingerprints bind the full original scope and matched API; the
        # normalization offset map retains exact source ownership and line.
        for rule, pattern in rules.items():
            for match in pattern.finditer(text):
                position = start + offsets[match.start()]
                if (rule, position) in seen: continue
                seen.add((rule, position))
                owners = [item for item in functions if start <= item[0] <= position < item[1] <= end]
                owner = min(owners, key=lambda item: item[1] - item[0]) if owners else (start, end, region_name)
                context = re.sub(r'\s+', ' ', content[owner[0]:owner[1]]).strip()
                if rule == 'timing-alias':
                    context += '\nALIAS_BINDING:' + aliases[match.group(1)]
                context_hash = hashlib.sha256(context.encode()).hexdigest()
                occurrence = sum(1 for previous in findings if previous.path == path and previous.scope == owner[2] and previous.rule == rule and previous.context_sha256 == context_hash)
                identity = [path, owner[2], context_hash, rule, match.group(), occurrence]
                key = hashlib.sha256(json.dumps(identity, separators=(',', ':')).encode()).hexdigest()
                line = source.count('\n', 0, position) + 1
                excerpt = source.splitlines()[line - 1].strip()[:180]
                findings.append(Finding(key, path, line, owner[2], rule, excerpt, context_hash))
    return sorted(findings, key=lambda item: (item.path, item.line, item.rule, item.key))


def read_policy(path: Path) -> dict:
    def unique_pairs(pairs):
        result = {}
        for key, value in pairs:
            if key in result: raise ValueError(f'duplicate policy key: {key}')
            result[key] = value
        return result
    policy = json.loads(path.read_text(), object_pairs_hook=unique_pairs)
    if policy.get('version') != 1: raise ValueError('unsupported timing policy version')
    helpers = policy.get('helpers', {})
    for path, reason in helpers.items():
        if not reason.strip() or '*' in path or Path(path).is_absolute() or '..' in Path(path).parts:
            raise ValueError('helpers require exact repository paths and a rationale')
    baseline = policy.get('migration_baseline')
    if baseline:
        for field, size in (('source_commit', 40), ('source_tree', 40), ('generator_sha256', 64), ('inventory_sha256', 64)):
            if not re.fullmatch(f'[0-9a-f]{{{size}}}', baseline.get(field, '')):
                raise ValueError(f'migration baseline requires immutable {field}')
        keys = baseline.get('fingerprints', [])
        if keys != sorted(set(keys)) or any(not re.fullmatch('[0-9a-f]{64}', key) for key in keys):
            raise ValueError('migration fingerprints must be unique, sorted exact identities')
        inventory_hash = hashlib.sha256(json.dumps(keys, separators=(',', ':')).encode()).hexdigest()
        if inventory_hash != baseline['inventory_sha256']:
            raise ValueError('migration inventory hash does not match its fingerprints')
        if baseline.get('count') != len(keys) or not baseline.get('tracking') or not baseline.get('note'):
            raise ValueError('migration baseline requires count, tracking and unresolved qualification')
    for key, item in policy.get('dispositions', {}).items():
        if not re.fullmatch('[0-9a-f]{64}', key) or item.get('category') not in CATEGORIES:
            raise ValueError('invalid timing disposition')
        if not item.get('reason', '').strip() or not item.get('tracking', '').strip():
            raise ValueError('every disposition needs specific reason and tracking')
        if item['category'] != 'unresolved' and not item.get('review', '').strip():
            raise ValueError('accepted classifications require an explicit review reference')
    return policy


def report(root: Path, policy: dict) -> dict:
    tracked = subprocess.check_output(['git', 'ls-files', '-z'], cwd=root).decode().split('\0')
    findings, failures = [], []
    helpers = set(policy.get('helpers', {}))
    sources = {path: (root / path).read_text() for path in tracked if Path(path).suffix in EXTENSIONS}
    pending = [path for path in sources if path.endswith('.rs')]
    discovered = set()
    while pending:
        path = pending.pop()
        try:
            targets, errors = external_test_modules(path, sources[path], root, helper=path in helpers)
            failures.extend({'path': path, 'error': error} for error in errors)
            for target in targets:
                if target not in sources:
                    failures.append({'path': path, 'error': f'external test module is not tracked: {target}'})
                elif target not in helpers:
                    helpers.add(target); discovered.add(target); pending.append(target)
        except ValueError as error: failures.append({'path': path, 'error': str(error)})
    for path, source in sources.items():
        try: findings.extend(scan(path, source, helper=path in helpers))
        except (ValueError, UnicodeError) as error: failures.append({'path': path, 'error': str(error)})
    for helper in helpers:
        if helper not in tracked: failures.append({'path': helper, 'error': 'registered helper missing from tracked files'})
    dispositions = policy.get('dispositions', {})
    baseline = policy.get('migration_baseline', {})
    baseline_keys = set(baseline.get('fingerprints', []))
    classified = []
    for finding in findings:
        item = asdict(finding)
        item['disposition'] = dispositions.get(finding.key)
        if item['disposition'] is None and finding.key in baseline_keys:
            item['disposition'] = {
                'category': 'unresolved', 'reason': baseline['note'],
                'tracking': baseline['tracking'], 'source_commit': baseline['source_commit'],
                **{field: getattr(finding, field) for field in ('path', 'scope', 'rule', 'context_sha256')},
            }
        if item['disposition']:
            for field in ('path', 'scope', 'rule', 'context_sha256'):
                if item['disposition'].get(field) != getattr(finding, field):
                    failures.append({'path': finding.path, 'error': f'disposition {finding.key} has mismatched {field}'})
        classified.append(item)
    return {'schema_version': 1, 'scope': 'lexical regression inventory, not completeness proof',
            'findings': classified, 'scan_errors': failures, 'discovered_test_modules': sorted(discovered),
            'new_unclassified': sum(item['disposition'] is None for item in classified),
            'unresolved_existing': sum((item['disposition'] or {}).get('category') == 'unresolved' for item in classified),
            'stale_dispositions': sorted(set(dispositions) - {item.key for item in findings}),
            'stale_migration_fingerprints': sorted(baseline_keys - {item.key for item in findings})}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--policy', type=Path)
    parser.add_argument('--report', type=Path)
    parser.add_argument('--require-resolved', action='store_true', help='closure audit: also reject unresolved migration findings')
    args = parser.parse_args()
    policy_path = args.policy or args.root / 'docs/test-timing-policy.json'
    try: result = report(args.root, read_policy(policy_path))
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f'timing guard configuration/scan failed: {error}', file=sys.stderr); return 2
    encoded = json.dumps(result, indent=2) + '\n'
    if args.report: args.report.write_text(encoded)
    else: print(encoded, end='')
    failed = bool(result['scan_errors'] or result['new_unclassified'] or args.require_resolved and result['unresolved_existing'])
    if result['unresolved_existing']:
        print(f"::warning title=Timing migration incomplete::{result['unresolved_existing']} existing findings remain unresolved under the migration policy.", file=sys.stderr)
    print(f"Timing regression report: {result['new_unclassified']} unclassified, {result['unresolved_existing']} unresolved existing; completion is NOT established.", file=sys.stderr)
    return int(failed)


if __name__ == '__main__':
    raise SystemExit(main())
