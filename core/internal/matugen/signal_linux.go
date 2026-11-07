package matugen

import (
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
)

func signalByName(name string, sig syscall.Signal) {
	entries, err := os.ReadDir("/proc")
	if err != nil {
		return
	}
	for _, entry := range entries {
		pid, err := strconv.Atoi(entry.Name())
		if err != nil {
			continue
		}
		comm, err := os.ReadFile(filepath.Join("/proc", entry.Name(), "comm"))
		if err != nil {
			continue
		}
		if strings.TrimSpace(string(comm)) == name {
			syscall.Kill(pid, sig)
		}
	}
}
