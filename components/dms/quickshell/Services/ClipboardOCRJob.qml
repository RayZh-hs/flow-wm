pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import Quickshell.Io
import qs.Common

Item {
    id: job

    required property int entryId
    property var command: ["python3", Quickshell.shellDir + "/scripts/clipboard-ocr.py", "--entry", String(entryId)]
    property int timeoutMs: 140000
    property bool completed: false

    signal finished(var result)

    function start() {
        watchdog.start();
        worker.running = true;
    }

    function finish(result) {
        if (completed)
            return;
        completed = true;
        watchdog.stop();
        finished(result);
        cleanup.start();
    }

    Process {
        id: worker
        command: job.command
        stdout: SplitParser {
            onRead: line => {
                if (!line.trim() || job.completed)
                    return;
                let result;
                try {
                    result = JSON.parse(line);
                    if (typeof result.ok !== "boolean")
                        throw new Error("Invalid OCR result");
                } catch (error) {
                    job.finish({ok: false, error: I18n.tr("The OCR helper returned an invalid result")});
                    return;
                }
                job.finish(result);
            }
        }
        stderr: StdioCollector {}
        onRunningChanged: {
            if (!running) {
                Qt.callLater(function () {
                    job.finish({ok: false, error: I18n.tr("The OCR helper stopped without a result")});
                    job.destroy();
                });
            }
        }
    }

    Timer {
        id: watchdog
        interval: job.timeoutMs
        onTriggered: {
            job.finish({ok: false, error: I18n.tr("Text extraction timed out")});
            worker.running = false;
        }
    }

    Timer {
        id: cleanup
        interval: 1000
        onTriggered: {
            if (worker.running)
                worker.running = false;
            else
                job.destroy();
        }
    }
}
