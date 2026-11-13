#!/usr/bin/env python3
"""Run GTK rendering/input unit tests against isolated production host/provider processes.

Run under Xvfb or a private display. Each GTK test needs its own process because
Rust's test runner initializes tests on different threads.
"""
import os
from pathlib import Path
import subprocess
import tempfile
from runtime import start

ROOT = Path(__file__).resolve().parents[1]
TESTS = (
    "ui::canvas::rendering_tests::constant_clicks_and_drags_release_input_and_commit_positions",
    "ui::canvas::rendering_tests::gpu_window_cards_keep_full_selection_outlines_and_foreground_gestures",
    "ui::gpu::tests::native_morphs_do_not_paint_opaque_boxes_behind_stationary_previews",
    "ui::gpu::tests::native_clips_match_mask_pixels_for_overlapping_translucent_cards",
    "ui::gpu::tests::workflow_cards_retain_drawing_during_motion_and_refresh_changed_content",
)


def main():
    children = []
    with tempfile.TemporaryDirectory(prefix="flowwm-rendering-") as temporary:
        runtime = Path(temporary)
        config = runtime / "config"
        config.mkdir()
        environment = os.environ.copy()
        environment.pop("NIRI_SOCKET", None)
        environment.update(XDG_RUNTIME_DIR=temporary, FLOWWM_CONFIG_DIR=str(config), GTK_A11Y="none")
        try:
            start(ROOT / "target/debug/flowwm", runtime, environment, children)
            for name in TESTS:
                subprocess.run(["cargo", "test", "-p", "flowwm", "--lib", name, "--locked",
                                "--", "--ignored", "--exact"], cwd=ROOT, env=environment, check=True)
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    child.terminate()
                    child.wait(timeout=10)


if __name__ == "__main__":
    main()
