import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('firmware_memory', Path(__file__).parents[1] / 'check-firmware-memory.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class MemoryBudgetTests(unittest.TestCase):
    def test_rejects_observed_failed_stack_and_accepts_working_images(self):
        for end, expected in [('3ffde5cc', 6708), ('3ffde304', 7420)]:
            result = module.check_symbols(f'{end} A _stack_end_cpu0\n3ffe0000 A _stack_start_cpu0\n', 6144)
            self.assertEqual(result['startup_stack_reserved_bytes'], expected)
        with self.assertRaisesRegex(ValueError, '3324 bytes'):
            module.check_symbols('3ffdf304 A _stack_end_cpu0\n3ffe0000 A _stack_start_cpu0\n', 6144)

    def test_seven_kib_gate_rejects_new_guard_failure(self):
        for size in (3324, 6556, 6708):
            with self.assertRaises(ValueError):
                module.check_symbols(f'{0x3ffe0000-size:x} A _stack_end_cpu0\n3ffe0000 A _stack_start_cpu0\n', 7168)
        self.assertEqual(module.check_symbols('3ffde3d4 A _stack_end_cpu0\n3ffe0000 A _stack_start_cpu0\n', 7168)['startup_stack_reserved_bytes'], 7212)

    def test_missing_duplicate_reversed_and_invalid_budgets_fail(self):
        cases = ['', '3ffe0000 A _stack_start_cpu0',
                 '3ffe0000 A _stack_start_cpu0\n3ffe0001 A _stack_end_cpu0',
                 '3ffe0000 A _stack_start_cpu0\n3ffe0000 A _stack_start_cpu0\n3ffde304 A _stack_end_cpu0']
        for value in cases:
            with self.subTest(value=value), self.assertRaises(ValueError):
                module.check_symbols(value, 6144)
        with self.assertRaises(ValueError):
            module.check_symbols('', 0)
