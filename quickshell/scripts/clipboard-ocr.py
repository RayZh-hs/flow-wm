#!/usr/bin/env python3
"""Local OCR for a DMS history image or an image file; emit one JSON result."""

import argparse
import base64
import binascii
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from PIL import Image, ImageOps, ImageStat, UnidentifiedImageError


class OCRError(Exception):
    pass


def run(command, *, operation="Text recognition", **kwargs):
    try:
        return subprocess.run(command, check=True, capture_output=True, timeout=30, **kwargs)
    except FileNotFoundError as exc:
        raise OCRError(f"Required program is missing: {command[0]}") from exc
    except subprocess.TimeoutExpired as exc:
        raise OCRError(f"{operation} timed out.") from exc
    except subprocess.CalledProcessError as exc:
        detail = (exc.stderr or b"").decode("utf-8", errors="replace").strip()
        raise OCRError(detail[:400] or f"{command[0]} failed (exit {exc.returncode}).") from exc


def history_image(entry_id):
    """Fetch the selected entry directly, without replacing the current clipboard."""
    result = json.loads(run(["dms", "clipboard", "get", str(entry_id), "--json"], operation="Reading the clipboard image").stdout)
    if not result or not result.get("isImage"):
        raise OCRError("This clipboard entry is unavailable or is not an image.")
    try:
        return base64.b64decode(result["data"], validate=True)
    except (KeyError, binascii.Error) as exc:
        raise OCRError("Could not decode this clipboard image.") from exc


def prepare_image(data, output):
    """Flatten transparency and enlarge small screenshot text for Tesseract."""
    try:
        with Image.open(io.BytesIO(data)) as source:
            if source.width * source.height > 40_000_000:
                raise OCRError("This image is too large. Crop it before extracting text.")
            source.load()
            rgba = ImageOps.exif_transpose(source).convert("RGBA")
    except (UnidentifiedImageError, OSError, Image.DecompressionBombError) as exc:
        raise OCRError("The selected image could not be opened.") from exc
    background = Image.new("RGBA", rgba.size, "white")
    background.alpha_composite(rgba)
    image = ImageOps.grayscale(background.convert("RGB"))
    # Dark screenshots need dark lettering on a light background.
    if ImageStat.Stat(image).median[0] < 110:
        image = ImageOps.invert(image)
    image = ImageOps.autocontrast(image)
    # Very large glyphs reduce recognition accuracy, especially for Chinese.
    scale = min(2.0, 1500 / max(image.size))
    if scale > 1:
        image = image.resize(tuple(round(side * scale) for side in image.size), Image.Resampling.LANCZOS)
    ImageOps.expand(image, border=16, fill="white").save(output)


def recognize(data, languages):
    data_home = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share"))
    tessdata = Path(os.environ.get("DMS_OCR_TESSDATA", data_home / "dms-ocr/tessdata"))
    missing = [lang for lang in languages.split("+") if not (tessdata / f"{lang}.traineddata").is_file()]
    if missing:
        raise OCRError("OCR language data missing: " + ", ".join(missing))
    with tempfile.TemporaryDirectory(prefix="dms-ocr-") as directory:
        image = Path(directory) / "image.png"
        prepare_image(data, image)
        command = ["tesseract", str(image), "stdout", "--tessdata-dir", str(tessdata),
                   "-l", languages, "--oem", "1"]
        result = run(command + ["--psm", "3"], env={**os.environ, "OMP_THREAD_LIMIT": "2"})
        text = result.stdout.decode("utf-8").strip()
        if not text:
            # Sparse labels/screenshots sometimes do not form a full text block.
            text = run(command + ["--psm", "11"], env={**os.environ, "OMP_THREAD_LIMIT": "2"}).stdout.decode("utf-8").strip()
    if not text:
        raise OCRError("No text found in this image.")
    return text


def copy_text(text):
    # wl-copy forks a clipboard owner that keeps stderr open until the next copy.
    # PIPE/capture_output would wait for that daemon, even after copying succeeds.
    # A regular file preserves startup diagnostics without waiting for pipe EOF.
    with tempfile.TemporaryFile() as errors:
        try:
            subprocess.run(
                ["wl-copy", "--type", "text/plain;charset=utf-8"],
                input=text.encode("utf-8"), stdout=subprocess.DEVNULL,
                stderr=errors, check=True, timeout=5,
            )
        except FileNotFoundError as exc:
            raise OCRError("Required program is missing: wl-copy") from exc
        except subprocess.TimeoutExpired as exc:
            raise OCRError("Copying text to the clipboard timed out.") from exc
        except subprocess.CalledProcessError as exc:
            errors.seek(0)
            detail = errors.read(4096).decode("utf-8", errors="replace").strip()
            raise OCRError("Could not copy text to the clipboard" + (f": {detail}" if detail else ".")) from exc


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--entry", type=int, help="DMS clipboard history entry ID")
    source.add_argument("--file", type=Path, help="Image file to recognize")
    parser.add_argument("--no-copy", action="store_true", help="Return text without changing the clipboard")
    parser.add_argument("--languages", default=os.environ.get("DMS_OCR_LANGUAGES", "chi_sim+eng"))
    args = parser.parse_args()
    try:
        if not shutil.which("tesseract"):
            raise OCRError("Install Tesseract to extract text from images.")
        data = history_image(args.entry) if args.entry is not None else args.file.read_bytes()
        text = recognize(data, args.languages)
        if not args.no_copy:
            copy_text(text)
        print(json.dumps({"ok": True, "text": text, "copied": not args.no_copy}, ensure_ascii=False), flush=True)
        return 0
    except (OCRError, OSError, ValueError) as exc:
        print(json.dumps({"ok": False, "error": str(exc)}, ensure_ascii=False), flush=True)
        return 1


if __name__ == "__main__":
    sys.exit(main())
