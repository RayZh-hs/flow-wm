"""Functional checks against real Tesseract; these never change the clipboard."""

import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest

from PIL import Image, ImageDraw, ImageFont


HELPER = Path(__file__).resolve().parents[2] / "quickshell/scripts/clipboard-ocr.py"


class OCRTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="test-dms-ocr-")
        self.addCleanup(self.directory.cleanup)
        self.image = Path(self.directory.name) / "test.png"

    def recognize(self, *extra):
        process = subprocess.run([sys.executable, str(HELPER), "--file", str(self.image), "--no-copy", *extra], capture_output=True, text=True, timeout=40)
        self.assertEqual(process.stderr, "")
        return process.returncode, json.loads(process.stdout)

    def draw(self, text, dark=False):
        font_path = subprocess.check_output(["fc-match", "Noto Sans CJK SC", "-f", "%{file}"], text=True)
        font = ImageFont.truetype(font_path, 36)
        image = Image.new("RGB", (1000, 240), "#1e1e1e" if dark else "white")
        ImageDraw.Draw(image).multiline_text((25, 25), text, font=font, fill="white" if dark else "black", spacing=12)
        image.save(self.image)

    def test_english(self):
        self.draw("Clipboard OCR integration\nProtein design and enzyme research\nExperiment ID: 123456789")
        code, result = self.recognize()
        self.assertEqual(code, 0)
        self.assertIn("Protein design and enzyme research", result["text"])
        self.assertIn("123456789", result["text"])
        self.assertFalse(result["copied"])

    def test_chinese(self):
        self.draw("剪贴板文字识别\n蛋白质设计与科学研究\n实验编号 123456789")
        code, result = self.recognize()
        self.assertEqual(code, 0)
        self.assertIn("剪贴板文字识别", result["text"])
        self.assertIn("蛋白质设计与科学研究", result["text"])

    def test_mixed_languages(self):
        self.draw("Clipboard OCR 文字识别\n蛋白质设计 Protein design 2026")
        code, result = self.recognize()
        self.assertEqual(code, 0)
        self.assertIn("文字识别", result["text"])
        self.assertIn("Protein design 2026", result["text"])

    def test_dark_screenshot(self):
        self.draw("Extract text from a dark screenshot\nClipboard OCR 2026", dark=True)
        code, result = self.recognize()
        self.assertEqual(code, 0)
        self.assertIn("Extract text from a dark screenshot", result["text"])

    def test_blank_image(self):
        Image.new("RGB", (500, 200), "white").save(self.image)
        code, result = self.recognize()
        self.assertEqual(code, 1)
        self.assertFalse(result["ok"])
        self.assertEqual(result["error"], "No text found in this image.")

    def test_invalid_image(self):
        self.image.write_text("not an image")
        code, result = self.recognize()
        self.assertEqual(code, 1)
        self.assertFalse(result["ok"])
        self.assertIn("could not be opened", result["error"])

    def test_missing_language(self):
        self.draw("Clipboard OCR")
        code, result = self.recognize("--languages", "missing-language")
        self.assertEqual(code, 1)
        self.assertFalse(result["ok"])
        self.assertIn("language data missing", result["error"])

    def fake_copy(self, code):
        executable = Path(self.directory.name) / "wl-copy"
        executable.write_text(f"#!{sys.executable}\n" + code)
        executable.chmod(0o700)
        return {**os.environ, "PATH": f"{self.directory.name}:{os.environ['PATH']}"}

    def test_copy_completes_while_background_owner_keeps_stderr_open(self):
        self.draw("Clipboard OCR integration")
        directory = Path(self.directory.name)
        pid_file = directory / "owner.pid"
        copied_file = directory / "copied.txt"
        environment = self.fake_copy(f"""import os, pathlib, sys, time
pathlib.Path({str(copied_file)!r}).write_bytes(sys.stdin.buffer.read())
if os.fork() == 0:
    pathlib.Path({str(pid_file)!r}).write_text(str(os.getpid()))
    time.sleep(20)  # Inherit stderr like wl-copy's long-lived clipboard owner.
os._exit(0)
""")
        try:
            process = subprocess.run([sys.executable, str(HELPER), "--file", str(self.image)], env=environment, capture_output=True, text=True, timeout=3)
            self.assertEqual(process.returncode, 0, process.stdout)
            result = json.loads(process.stdout)
            self.assertTrue(result["copied"])
            self.assertEqual(copied_file.read_text(), result["text"])
        finally:
            if pid_file.exists():
                try:
                    os.kill(int(pid_file.read_text()), signal.SIGTERM)
                except ProcessLookupError:
                    pass

    def test_copy_failure_preserves_diagnostics(self):
        self.draw("Clipboard OCR integration")
        environment = self.fake_copy('import sys\nprint("Wayland unavailable", file=sys.stderr)\nsys.exit(7)\n')
        process = subprocess.run([sys.executable, str(HELPER), "--file", str(self.image)], env=environment, capture_output=True, text=True, timeout=3)
        self.assertEqual(process.returncode, 1)
        result = json.loads(process.stdout)
        self.assertFalse(result["ok"])
        self.assertIn("Wayland unavailable", result["error"])


if __name__ == "__main__":
    unittest.main()
