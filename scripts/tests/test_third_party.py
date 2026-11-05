"""Pruebas del inventario de terceros (D60) con metadatos sintéticos; no ejecutan cargo."""
import re
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import third_party  # noqa: E402


def package(pid, name, manifest, source="registry+https://github.com/rust-lang/crates.io-index",
            license_="MIT"):
    return {"id": pid, "name": name, "version": "1.0.0", "source": source,
            "license": license_, "manifest_path": str(manifest), "repository": None}


def dep(pid, kind=None):
    return {"pkg": pid, "dep_kinds": [{"kind": kind, "target": None}]}


class LinkedCrates(unittest.TestCase):
    def metadata(self, root):
        return {
            "packages": [
                package("app", "nestadmin-desktop", root / "app/Cargo.toml", source=None),
                package("core", "nestadmin-core", root / "core/Cargo.toml", source=None),
                package("serde", "serde", root / "serde/Cargo.toml", license_="MIT OR Apache-2.0"),
                package("cc", "cc", root / "cc/Cargo.toml"),
                package("tempfile", "tempfile", root / "tempfile/Cargo.toml"),
                package("libc", "libc", root / "libc/Cargo.toml"),
            ],
            "resolve": {"nodes": [
                {"id": "app", "deps": [dep("core"), dep("cc", "build"), dep("tempfile", "dev")]},
                {"id": "core", "deps": [dep("serde")]},
                {"id": "serde", "deps": [dep("libc")]},
                {"id": "cc", "deps": []},
                {"id": "tempfile", "deps": []},
                {"id": "libc", "deps": []},
            ]},
        }

    def test_only_external_normal_dependencies_are_listed(self):
        crates = third_party.linked_crates(self.metadata(Path("x")))
        self.assertEqual([c["name"] for c in crates], ["libc", "serde"])

    def test_identical_license_texts_are_written_once(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name in ("serde", "libc"):
                (root / name).mkdir()
                (root / name / "LICENSE-MIT").write_text("Texto MIT sintetico\n", "utf-8")
            (root / "libc" / "README.md").write_text("no es licencia", "utf-8")
            vendor = root / "vendor"
            (vendor / "pdfium/licenses").mkdir(parents=True)
            (vendor / "pdfium/LICENSE").write_text("pdfium", "utf-8")
            (vendor / "pdfium/licenses/zlib.txt").write_text("zlib", "utf-8")
            (vendor / "tesseract").mkdir()
            (vendor / "tesseract/LICENSE").write_text("apache", "utf-8")
            crates = third_party.linked_crates(self.metadata(root))
            texts, missing = third_party.full_texts(crates, vendor)
        self.assertEqual(missing, [])
        self.assertEqual(texts.count("Texto MIT sintetico"), 1)
        self.assertIn("libc 1.0.0 (LICENSE-MIT), serde 1.0.0 (LICENSE-MIT)", texts)
        self.assertNotIn("no es licencia", texts)

    def test_missing_files_are_reported(self):
        with tempfile.TemporaryDirectory() as tmp:
            crates = third_party.linked_crates(self.metadata(Path(tmp)))
            _, missing = third_party.full_texts(crates, Path(tmp) / "vendor")
        self.assertIn("Tesseract OCR (vendor/tesseract/LICENSE)", missing)
        self.assertIn("serde 1.0.0 (MIT OR Apache-2.0)", missing)


class Render(unittest.TestCase):
    def test_npm_dev_dependencies_are_excluded(self):
        lock = {"packages": {
            "": {"name": "ui"},
            "node_modules/@tauri-apps/api": {"version": "2.12.4", "license": "Apache-2.0 OR MIT"},
            "node_modules/vite": {"version": "8.3.5", "license": "MIT", "dev": True},
        }}
        self.assertEqual(third_party.npm_production(lock),
                         [{"name": "@tauri-apps/api", "version": "2.12.4",
                           "license": "Apache-2.0 OR MIT"}])

    def test_summary_lists_every_component_and_escapes_tables(self):
        crates = [{"name": "a", "version": "1", "license": "MIT|X", "repository": None}]
        text = third_party.render(crates, [])
        for name, *_ in third_party.COMPONENTS:
            self.assertIn(name, text)
        for dll, *_ in third_party.TESSERACT_DLLS:
            self.assertIn(dll, text)
        self.assertIn("MIT\\|X", text)
        self.assertTrue(text.endswith("\n"))

    def test_dlls_match_setup_ocr(self):
        script = (third_party.ROOT / "scripts/setup-ocr.ps1").read_text("utf-8-sig")
        copied = set(re.findall(r"'([\w+.-]+\.dll)'", script)) - {"pdfium.dll"}
        self.assertEqual({dll for dll, *_ in third_party.TESSERACT_DLLS}, copied)


if __name__ == "__main__":
    unittest.main()
