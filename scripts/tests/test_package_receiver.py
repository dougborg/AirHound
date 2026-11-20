import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location("package_receiver", Path(__file__).parents[1] / "package-receiver.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class CandidateChecks(unittest.TestCase):
    def test_dependency_check_rejects_homebrew_and_relative_libraries(self):
        MODULE.check_macos_dependencies("binary:\n\t/usr/lib/libSystem.B.dylib (version)\n")
        for library in ("/opt/homebrew/opt/libusb/lib/libusb.dylib", "@rpath/libusb.dylib"):
            with self.assertRaises(ValueError):
                MODULE.check_macos_dependencies("binary:\n\t" + library + " (version)\n")

    def test_vendor_config_remains_relocatable(self):
        config = '[source.crates-io]\nreplace-with = "vendored-sources"\n[source.vendored-sources]\ndirectory = "/private/tmp/staging/vendor"\n'
        result = MODULE.vendor_config(config)
        self.assertNotIn("/private/tmp", result)
        self.assertIn('directory = "vendor"', result)
        with self.assertRaises(ValueError):
            MODULE.vendor_config("[source.crates-io]\n")


if __name__ == "__main__":
    unittest.main()
