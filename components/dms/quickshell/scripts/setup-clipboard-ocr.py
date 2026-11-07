#!/usr/bin/env python3
"""Install local OCR language models and optionally activate this DMS checkout."""

import argparse
import hashlib
import importlib.util
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


MODELS = {
    "eng": "7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2",
    "chi_sim": "a5fcb6f0db1e1d6d8522f39db4e848f05984669172e584e8d76b6b3141e1f730",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--activate", action="store_true", help="Select this checkout in the user DMS service")
    parser.add_argument("--restart", action="store_true", help="Restart DMS after activation")
    args = parser.parse_args()
    if args.restart and not args.activate:
        parser.error("--restart requires --activate")
    missing = [program for program in ("dms", "tesseract", "wl-copy", "python3", "curl") if not shutil.which(program)]
    if importlib.util.find_spec("PIL") is None:
        missing.append("Python Pillow")
    if missing:
        parser.error("Install the missing dependencies: " + ", ".join(missing))
    shell = Path(__file__).resolve().parents[1]
    if not (shell / "DCommon/Common/Style.qml").is_file():
        parser.error("Initialize the shared widgets first: git submodule update --init --recursive")
    if args.activate:
        expected_major = (shell / "VERSION").read_text().strip().lstrip("v").split(".")[0]
        version = subprocess.run(["dms", "version"], capture_output=True, text=True, check=True)
        installed = re.search(r"\bv?(\d+)\.\d+", version.stdout)
        if not installed or installed.group(1) != expected_major:
            parser.error(f"This checkout requires a DMS {expected_major}.x backend; install it before activating the interface")
    data_home = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share"))
    tessdata = Path(os.environ.get("DMS_OCR_TESSDATA", data_home / "dms-ocr/tessdata"))
    tessdata.mkdir(parents=True, exist_ok=True)
    for language, checksum in MODELS.items():
        model = tessdata / f"{language}.traineddata"
        if model.is_file() and hashlib.sha256(model.read_bytes()).hexdigest() == checksum:
            continue
        with tempfile.TemporaryDirectory(dir=tessdata) as temporary:
            download = Path(temporary) / model.name
            url = f"https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/4.1.0/{model.name}"
            subprocess.run(["curl", "-fLsS", "--retry", "2", "--max-time", "90", url, "-o", str(download)], check=True)
            if hashlib.sha256(download.read_bytes()).hexdigest() != checksum:
                raise RuntimeError(f"Checksum mismatch for {model.name}")
            download.replace(model)
    print(f"OCR models ready: {tessdata}")
    if not args.activate:
        return
    config_home = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))
    dropin = config_home / "systemd/user/dms.service.d/ocr.conf"
    dropin.parent.mkdir(parents=True, exist_ok=True)
    # systemd expands percent specifiers even inside quoted environment values.
    value = str(shell).replace("%", "%%").replace("\\", "\\\\").replace('"', '\\"')
    dropin.write_text(f'[Service]\nEnvironment="DMS_SHELL_DIR={value}"\n')
    subprocess.run(["systemctl", "--user", "daemon-reload"], check=True)
    if args.restart:
        subprocess.run(["systemctl", "--user", "restart", "dms"], check=True)
    print(f"DMS interface selected: {shell}")
    if not args.restart:
        print("Activate with: systemctl --user restart dms")


if __name__ == "__main__":
    main()
