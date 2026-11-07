package main

import (
	"errors"
	"slices"
	"strings"
	"testing"
)

func TestTrySystemdRestart(t *testing.T) {
	t.Setenv("FLOWWM_SESSION", "")
	tests := []struct {
		name        string
		running     bool
		ok          []string
		wantHandled bool
		wantCall    string
	}{
		{"active unit restarts", false, []string{"is-active dms.service", "restart"}, true, "restart dms.service"},
		{"enabled inactive unit starts", false, []string{"is-enabled", "is-active graphical-session.target", "start"}, true, "start dms.service"},
		{"hand-started instance keeps plain restart", true, []string{"is-enabled", "is-active graphical-session.target", "start"}, false, ""},
		{"session target not up", false, []string{"is-enabled"}, false, ""},
		{"unit disabled", false, []string{"is-active graphical-session.target"}, false, ""},
		{"no systemd", false, nil, false, ""},
	}

	origSystemctl, origRunning := runSystemctl, shellRunning
	t.Cleanup(func() { runSystemctl, shellRunning = origSystemctl, origRunning })

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			shellRunning = func() bool { return tt.running }
			var calls []string
			runSystemctl = func(args ...string) error {
				call := strings.Join(slices.DeleteFunc(slices.Clone(args), func(a string) bool {
					return strings.HasPrefix(a, "--")
				}), " ")
				calls = append(calls, call)
				for _, prefix := range tt.ok {
					if strings.HasPrefix(call, prefix) {
						return nil
					}
				}
				return errors.New("exit status 3")
			}

			handled, err := trySystemdRestart()
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if handled != tt.wantHandled {
				t.Fatalf("handled = %v, want %v (calls %q)", handled, tt.wantHandled, calls)
			}
			if tt.wantCall != "" && !slices.Contains(calls, tt.wantCall) {
				t.Fatalf("missing %q in calls %q", tt.wantCall, calls)
			}
			if !tt.wantHandled && slices.ContainsFunc(calls, func(c string) bool { return strings.HasPrefix(c, "start") }) {
				t.Fatalf("unexpected start in calls %q", calls)
			}
		})
	}
}

func TestFlowWMRestartIsIsolated(t *testing.T) {
	t.Setenv("FLOWWM_SESSION", "1")
	original, originalRunning := runSystemctl, shellRunning
	t.Cleanup(func() { runSystemctl, shellRunning = original, originalRunning })
	shellRunning = func() bool { return false }
	for _, active := range []bool{true, false} {
		var calls []string
		runSystemctl = func(args ...string) error {
			call := strings.Join(args, " ")
			calls = append(calls, call)
			if !active {
				return errors.New("inactive")
			}
			return nil
		}
		handled, err := trySystemdRestart()
		if err != nil || handled != active {
			t.Fatalf("active=%v: handled=%v err=%v calls=%q", active, handled, err, calls)
		}
		for _, call := range calls {
			if strings.HasSuffix(call, " dms.service") {
				t.Fatalf("FlowWM touched Niri's DMS: %s", call)
			}
		}
		if active && !slices.Contains(calls, "--user restart flowwm-dms.service") {
			t.Fatalf("did not restart FlowWM shell: %q", calls)
		}
		if !active && slices.ContainsFunc(calls, func(c string) bool { return strings.Contains(c, " start ") }) {
			t.Fatalf("started FlowWM shell outside its desktop: %q", calls)
		}
	}
}
