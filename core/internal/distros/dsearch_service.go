package distros

import (
	"context"
	"fmt"
	"os/exec"
)

// SetupDsearchService enables the dsearch.service user unit. Enablement failures
// are returned for the caller to surface as a non-fatal warning.
func SetupDsearchService(ctx context.Context, logf func(string)) error {
	if logf == nil {
		logf = func(string) {}
	}

	if err := runSystemctlUser(ctx, "daemon-reload"); err != nil {
		return err
	}

	if err := runSystemctlUser(ctx, "enable", "--now", "dsearch.service"); err != nil {
		return err
	}
	logf("Enabled dsearch.service")

	return nil
}

func runSystemctlUser(ctx context.Context, args ...string) error {
	cmd := exec.CommandContext(ctx, "systemctl", append([]string{"--user"}, args...)...)
	if output, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("systemctl --user %v failed: %w: %s", args, err, string(output))
	}
	return nil
}
