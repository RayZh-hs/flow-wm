"""Exercise the OCR job with Qt Test without launching a desktop shell."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class QMLJobTests(unittest.TestCase):
    def test_result_exit_failure_and_timeout_lifecycle(self):
        runner = next((path for name in ("qmltestrunner6", "/usr/lib/qt6/bin/qmltestrunner", "qmltestrunner") if (path := shutil.which(name))), None)
        self.assertIsNotNone(runner, "Qt 6 qmltestrunner is required")
        with tempfile.TemporaryDirectory(prefix="dms-ocr-qml-") as temporary:
            directory = Path(temporary)
            imports = directory / "imports"
            common = imports / "qs/Common"
            common.mkdir(parents=True)
            (common / "qmldir").write_text("module qs.Common\nsingleton I18n 1.0 I18n.qml\n")
            (common / "I18n.qml").write_text('pragma Singleton\nimport QtQml\nQtObject { function tr(text) { return text; } }\n')
            module = imports / "OCR"
            module.mkdir()
            (module / "qmldir").write_text("module OCR\nClipboardOCRJob 1.0 ClipboardOCRJob.qml\n")
            job_source = Path(__file__).resolve().parents[2] / "quickshell/Services/ClipboardOCRJob.qml"
            (module / "ClipboardOCRJob.qml").symlink_to(job_source)
            quickshell = imports / "Quickshell"
            io = quickshell / "Io"
            io.mkdir(parents=True)
            (quickshell / "qmldir").write_text("module Quickshell\nsingleton Quickshell 1.0 Quickshell.qml\n")
            (quickshell / "Quickshell.qml").write_text('pragma Singleton\nimport QtQml\nQtObject { property string shellDir: "" }\n')
            (io / "qmldir").write_text("module Quickshell.Io\nProcess 1.0 Process.qml\nSplitParser 1.0 SplitParser.qml\nStdioCollector 1.0 StdioCollector.qml\n")
            (io / "SplitParser.qml").write_text('import QtQml\nQtObject { signal read(string line) }\n')
            (io / "StdioCollector.qml").write_text('import QtQml\nQtObject {}\n')
            (io / "Process.qml").write_text('''import QtQml
QtObject {
    property var command: []
    property bool running: false
    property QtObject stdout
    property QtObject stderr
    onRunningChanged: {
        if (!running)
            return;
        const mode = command[0];
        if (mode === "success")
            stdout.read(JSON.stringify({ok: true}));
        else if (mode === "invalid")
            stdout.read("invalid-json");
        else if (mode === "exit" || mode === "/does/not/exist")
            running = false;
    }
}
''')
            source = '''import QtQuick
import QtTest
import OCR 1.0

TestCase {
    id: test
    name: "ClipboardOCRJob"
    property var job: null

    Component { id: factory; ClipboardOCRJob {} }
    SignalSpy { id: resultSpy; signalName: "finished" }

    function create(mode, timeoutMs) {
        job = factory.createObject(test, {
            entryId: 1,
            command: [mode],
            timeoutMs: timeoutMs || 10000
        });
        verify(job !== null);
        resultSpy.target = job;
        resultSpy.clear();
    }

    function cleanup() {
        resultSpy.target = null;
        if (job)
            job.destroy();
        job = null;
    }

    function test_result_before_exit_and_duplicate_completion() {
        create("success");
        job.start();
        tryCompare(resultSpy, "count", 1);
        compare(resultSpy.signalArguments[0][0].ok, true);
        compare(job.completed, true);
        job.finish({ok: false, error: "late failure"});
        compare(resultSpy.count, 1);
    }

    function test_failures_data() {
        return [
            {tag: "exit without result", mode: "exit", error: "stopped without a result", timeoutMs: 10000},
            {tag: "invalid output", mode: "invalid", error: "invalid result", timeoutMs: 10000},
            {tag: "watchdog", mode: "hang", error: "timed out", timeoutMs: 1},
            {tag: "failed launch", mode: "missing", error: "stopped without a result", timeoutMs: 10000}
        ];
    }

    function test_failures(data) {
        create(data.mode, data.timeoutMs);
        if (data.mode === "missing")
            job.command = ["/does/not/exist"];
        job.start();
        tryCompare(resultSpy, "count", 1);
        compare(resultSpy.count, 1);
        compare(resultSpy.signalArguments[0][0].ok, false);
        verify(resultSpy.signalArguments[0][0].error.indexOf(data.error) !== -1);
    }

    function test_independent_requests() {
        create("success");
        job.start();
        tryCompare(resultSpy, "count", 1);
        const previousJob = job;
        create("hang");
        job.start();
        previousJob.finish({ok: false, error: "stale result"});
        previousJob.destroy();
        compare(job.completed, false);
        compare(resultSpy.count, 0);
        job.finish({ok: true});
        compare(resultSpy.count, 1);
        compare(resultSpy.signalArguments[0][0].ok, true);
    }
}
'''
            target = directory / "tst_ClipboardOCRJob.qml"
            target.write_text(source)
            process = subprocess.run([runner, "-import", str(imports), "-input", str(target), "-o", "-,txt"], env={**os.environ, "QT_QPA_PLATFORM": "offscreen"}, capture_output=True, text=True, timeout=10)
            output = process.stdout + process.stderr
            self.assertEqual(process.returncode, 0, output)
            self.assertIn("0 failed", output)
            self.assertNotIn("TypeError", output)
            self.assertNotIn("ReferenceError", output)


if __name__ == "__main__":
    unittest.main()
