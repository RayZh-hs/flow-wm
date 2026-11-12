"""Independent host/provider composition shared by nested sessions and smoke runners."""
from pathlib import Path
import subprocess
from smoke_native import request, wait


def start(application, runtime, environment, children, log=None):
    application, runtime = Path(application), Path(runtime)
    host_socket, provider_socket = runtime / "flow-core.sock", runtime / "flowwm-desktop.sock"
    environment.update(FLOWWM_HOST_SOCKET=str(host_socket), FLOWWM_DESKTOP_SOCKET=str(provider_socket))
    host = subprocess.Popen([str(application.with_name("flow-core")), "--socket", str(host_socket),
                             "--data-dir", str(runtime / "data")], env=environment, stdout=log, stderr=log)
    children.append(host)
    wait(lambda: request(host_socket, {"protocol":3,"id":"startup","command":{"command":"info"}}), host)
    provider = subprocess.Popen([str(application.with_name("flowwm-desktop"))], env=environment, stdout=log, stderr=log)
    children.append(provider)
    wait(lambda: request(provider_socket, {"desktop_protocol":1,"command":"bindings"}), provider)
    return host_socket
