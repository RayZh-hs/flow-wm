#!/usr/bin/env python3
"""A real application window for isolated native-preview smoke tests."""
import argparse
import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk, Gio

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("title")
args = parser.parse_args()
app = Gtk.Application(application_id="dev.flowwm.test.preview", flags=Gio.ApplicationFlags.NON_UNIQUE)


def activate(application):
    window = Gtk.ApplicationWindow(application=application, title=args.title,
                                   default_width=720, default_height=850)
    window.set_child(Gtk.Label(label="Native application preview"))
    window.present()


app.connect("activate", activate)
app.run([])
