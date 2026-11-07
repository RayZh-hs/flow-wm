package matugen

import (
	"os/exec"
	"strings"
	"syscall"

	"golang.org/x/sys/unix"
)

// procfs(5) is optional on FreeBSD; pkill(1) from base queries the kernel
// directly.
func signalByName(name string, sig syscall.Signal) {
	signame := strings.TrimPrefix(unix.SignalName(sig), "SIG")
	exec.Command("pkill", "-"+signame, "-x", name).Run()
}
