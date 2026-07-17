package cli

import (
	"fmt"

	"github.com/spf13/cobra"
)

// newRuntimeCmd runs the resident daemon: direct-link cache, circuit
// breaker and quotas, serving scanner/transcoder over UDS.
func newRuntimeCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "runtime",
		Short: "Run the resident link-cache daemon",
		RunE: func(*cobra.Command, []string) error {
			return fmt.Errorf("runtime: not implemented")
		},
	}
}
