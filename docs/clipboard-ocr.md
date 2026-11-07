# Clipboard OCR fork

This fork adds local text extraction to DMS clipboard image entries. Its
`clipboard-ocr` branch is rebased onto upstream `master`, currently version
**2.0**, and requires a matching DMS 2.x backend. The fork's `master` branch also
includes the OCR integration. The upstream GitHub repository has moved from
`AvengeMedia/DankMaterialShell` to `duxlabs-dev/dux-shell`.

## Install

Install a matching DMS 2.x backend, Quickshell, Python 3, Pillow, Tesseract, `wl-clipboard`, and
`curl`. On Arch, the extra OCR dependencies are:

```sh
sudo pacman -S python-pillow tesseract wl-clipboard curl
```

Clone this branch with its pinned widget submodule:

```sh
git clone --branch clipboard-ocr --recurse-submodules \
    https://github.com/RayZh-hs/DankMaterialShell.git
cd DankMaterialShell
python3 quickshell/scripts/setup-clipboard-ocr.py --activate --restart
```

The setup command checks backend compatibility, downloads and verifies the English and Simplified Chinese
models, selects this checkout's `quickshell/` directory through the user
`dms.service` drop-in, and restarts DMS. Keep the checkout in place while using
it. The OCR helper is located relative to the shell directory and does not
depend on a separate installation or a fixed home-directory path.

The models occupy about 6.6 MB in
`${XDG_DATA_HOME:-~/.local/share}/dms-ocr/tessdata/`. They are reused if already
present and valid. Recognition runs offline on the CPU and never uploads images.
The setup command without `--activate` installs only the models.

You can also build the fork's embedded binary using the normal `make build`
workflow. Its embedded interface includes the OCR helper; install the Python
dependencies and run the model setup command separately.

## Use

Open clipboard history and use any of these:

- Click the **Tt** button on an image row.
- Right-click an image and choose **Extract text (OCR)**.
- Select an image with the arrow keys and press **Ctrl+O**.

The extracted text is copied to the clipboard and appears in normal history.
The image remains in history, subject to the existing history limit. The action
does not automatically paste into the focused application. A single request
runs at a time, and its button shows an hourglass until a result or error arrives.
Failed or empty recognition leaves the clipboard unchanged.

The engine is Tesseract LSTM (`--oem 1`), using the official
[`tessdata_fast` 4.1.0 models](https://github.com/tesseract-ocr/tessdata_fast/tree/4.1.0)
in `chi_sim+eng` order. It works best with printed text and screenshots.
Handwriting, equations, small blurry lettering, and complex layouts may need
manual correction.

Environment variables accepted by the helper:

| Variable | Default | Purpose |
| --- | --- | --- |
| `DMS_OCR_LANGUAGES` | `chi_sim+eng` | Tesseract language list |
| `DMS_OCR_TESSDATA` | XDG data directory's `dms-ocr/tessdata` | Language model directory |

For graphical use, set these in a user `dms.service` drop-in and restart DMS.
Additional languages need their corresponding `.traineddata` files.

## CLI and tests

```sh
# Use the same action as the GUI.
dms ipc call clipboard extractText ENTRY_ID
dms ipc call clipboard ocrStatus

# Inspect OCR without changing the clipboard.
python3 quickshell/scripts/clipboard-ocr.py --file screenshot.png --no-copy

# Extract a history image and copy the result.
python3 quickshell/scripts/clipboard-ocr.py --entry ENTRY_ID

# Run regression tests without touching the real clipboard.
python3 -m unittest discover -s tests/clipboard-ocr -v
```

The tests require the language models, Pillow, Tesseract, Qt 6 qmltestrunner,
Fontconfig, and the Noto Sans CJK font. The QML test loads the OCR job directly
with mocked process I/O and checks its signals through Qt Test without fixed
delays. It does not launch the desktop shell, a display server, or a compositor.

## Completion handling

The helper redirects `wl-copy` stderr to a temporary regular file rather than
capturing it through a pipe. This avoids waiting for the background clipboard
owner to exit after copying has succeeded. Startup errors retain their
diagnostics, and copying has a separate five-second timeout.

Each request owns its process and timers in `ClipboardOCRJob.qml`. A JSON result
completes the request immediately, independently of process exit or stream EOF.
An old process exiting cannot clear a newer request. Invalid output, failed
launches, exits without a result, and watchdog timeouts all complete the request
once. Processes and timers exist only while handling a request or cleaning it up.

Regression coverage includes English, Chinese, mixed text, dark images, blank
and invalid images, missing language data, the inherited-stderr failure,
clipboard error diagnostics, and QML completion/error/timeout handling.

## Update or disable

To update this fork while retaining its pinned widget dependency:

```sh
git pull --ff-only --recurse-submodules
systemctl --user restart dms
```

Upstream changes can be merged from the `upstream` remote. Upgrading to a newer
DMS major version requires checking compatibility with its backend and QML APIs.

To return to the package's embedded UI:

```sh
rm ~/.config/systemd/user/dms.service.d/ocr.conf
systemctl --user daemon-reload
systemctl --user restart dms
```
