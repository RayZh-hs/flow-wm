#!/usr/bin/env python3
"""A real application window for isolated native-preview smoke tests."""
import argparse
import gi
import json
from pathlib import Path

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, Gtk, Gio

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("title")
parser.add_argument("--key-log", type=Path, help="Record key presses for shortcut forwarding checks")
args = parser.parse_args()
app = Gtk.Application(application_id="dev.flowwm.test.preview", flags=Gio.ApplicationFlags.NON_UNIQUE)


def activate(application):
    window = Gtk.ApplicationWindow(application=application, title=args.title,
                                   default_width=720, default_height=850)
    content = Gtk.Label(label="Native application preview")
    window.set_child(content)
    if args.key_log:
        content.set_focusable(True)
        key = Gtk.EventControllerKey.new()
        key.set_propagation_phase(Gtk.PropagationPhase.CAPTURE)
        def pressed(controller, keyval, keycode, modifiers):
            with args.key_log.open("a") as stream:
                stream.write(json.dumps({"key": Gdk.keyval_name(keyval),
                                         "ctrl": bool(modifiers & Gdk.ModifierType.CONTROL_MASK),
                                         "shift": bool(modifiers & Gdk.ModifierType.SHIFT_MASK)}) + "\n")
            return False
        key.connect("key-pressed", pressed)
        window.add_controller(key)
    window.present()
    if args.key_log:
        content.grab_focus()


app.connect("activate", activate)
app.run([])
