# Clipboard OCR

This fork adds local English and Simplified Chinese text extraction to DMS
clipboard image entries, using Tesseract. FlowWM bundles the patched shell and
its matching backend in every installed snapshot.

## FlowWM setup

Install Python 3, Pillow, Tesseract, `wl-clipboard`, and `curl`. On Arch:

```sh
sudo pacman -S python-pillow tesseract wl-clipboard curl
```

From the FlowWM repository, initialize the shared widgets and install the models:

```sh
python3 scripts/components.py bootstrap
python3 components/dms/quickshell/scripts/setup-clipboard-ocr.py
```

Install FlowWM normally to include the patched UI and embedded helper. To build
and reload the desktop in an active FlowWM session:

```sh
python3 scripts/dev.py build all --release
python3 scripts/install.py --skip-build --replace
```

FlowWM uses `flowwm-dms.service` and the shell in its installed snapshot. Model
setup needs no `--activate` or `--restart`: those options are for a standalone
`dms.service` installation. FlowWM does not need a separate checkout or an OCR
service override.

OCR is enabled by default. In **Settings → Clipboard → Advanced**, toggle
**Extract text (OCR)** or change **OCR shortcut**. These settings live in
`~/.config/flow-wm/dms/settings.json`:

```json
{
  "clipboardOcrEnabled": true,
  "clipboardOcrShortcut": "Ctrl+O"
}
```

Merge these keys into existing settings. Use `Ctrl+key` or `Ctrl+Shift+key` for
the shortcut; an empty string disables only the shortcut. Disabling OCR hides
the image action and context-menu item and rejects new IPC requests.

## Standalone DMS setup

Install a matching DMS 2.x backend, Quickshell, Python 3, Pillow, Tesseract, `wl-clipboard`, and
`curl`. On Arch, the extra OCR dependencies are:

```sh
sudo pacman -S python-pillow tesseract wl-clipboard curl
```

Clone the patched fork with its pinned widget submodule:

```sh
git clone --recurse-submodules \
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

For graphical use, set these in a user `flowwm-dms.service` drop-in (or
`dms.service` for standalone DMS) and restart that service.
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

In FlowWM, `python3 scripts/dev.py test dms` runs these tests together with the
FlowWM catalog tests. Paths in the standalone examples above are relative to
`components/dms/` when using the FlowWM checkout.

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

FlowWM records the OCR integration as local changes on its DMS subtree. After
updating DMS with `scripts/components.py`, run `python3 scripts/dev.py test dms`
before building and installing the next snapshot. The tests require the OCR
helper and QML job, so losing the patch fails the standard DMS checks.

For a standalone checkout, update while retaining its pinned widget dependency:

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
