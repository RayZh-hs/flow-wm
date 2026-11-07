package trayrecovery

import "time"

// FreeBSD has no CLOCK_BOOTTIME (clock_gettime(2)), so suspend time since
// boot cannot be measured; the startup recovery scan is skipped.
func timeSuspended() time.Duration {
	return 0
}
