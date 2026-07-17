package cli

import (
	"fmt"

	"github.com/spf13/cobra"
)

// newInstallCmd symlinks the Plex Scanner/Transcoder to this binary,
// records VERSION metadata, and sets install-time Plex preferences.
func newInstallCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "install",
		Short: "Install scanner/transcoder symlinks into Plex",
		RunE: func(*cobra.Command, []string) error {
			return fmt.Errorf("install: not implemented")
		},
	}
	cmd.AddCommand(&cobra.Command{
		Use:   "verify",
		Short: "Verify installation integrity",
		RunE: func(*cobra.Command, []string) error {
			return fmt.Errorf("install verify: not implemented")
		},
	})
	return cmd
}
