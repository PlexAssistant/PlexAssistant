package cli

import (
	"fmt"

	"github.com/spf13/cobra"
)

// newTranscoderCmd is the thin Transcoder wrapper: refreshes expired direct
// links in the args, then execs the real "Plex Transcoder".
func newTranscoderCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "transcoder",
		Short: "Run as the Plex Transcoder wrapper",
		RunE: func(*cobra.Command, []string) error {
			return fmt.Errorf("transcoder: not implemented")
		},
	}
}
