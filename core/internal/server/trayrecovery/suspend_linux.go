package trayrecovery

import (
	"time"

	"golang.org/x/sys/unix"
)

// timeSuspended returns how long the system has spent in suspend since boot.
// It is the difference between CLOCK_BOOTTIME (includes suspend) and
// CLOCK_MONOTONIC (excludes suspend).
func timeSuspended() time.Duration {
	var bt, mt unix.Timespec
	if err := unix.ClockGettime(unix.CLOCK_BOOTTIME, &bt); err != nil {
		return 0
	}
	if err := unix.ClockGettime(unix.CLOCK_MONOTONIC, &mt); err != nil {
		return 0
	}
	diff := (bt.Sec-mt.Sec)*int64(time.Second) + (bt.Nsec - mt.Nsec)
	if diff < 0 {
		return 0
	}
	return time.Duration(diff)
}
