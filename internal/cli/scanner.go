package cli

import (
	"fmt"

	"github.com/spf13/cobra"
)

// newScannerCmd is the Plex Scanner replacement: invoked by Plex as
// `--analyze --section N --directory PATH`, it analyzes STRM targets with
// ffprobe and returns MediaSourceInfo XML.
func newScannerCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "scanner",
		Short: "Run as the Plex Scanner replacement",
		RunE: func(*cobra.Command, []string) error {
			return fmt.Errorf("scanner: not implemented")
		},
	}
	cmd.Flags().Bool("analyze", false, "analyze mode")
	cmd.Flags().Int("section", 0, "library section id")
	cmd.Flags().String("directory", "", "directory to scan")
	// Tolerate flags we do not know about yet.
	cmd.FParseErrWhitelist = cobra.FParseErrWhitelist{UnknownFlags: true}
	return cmd
}
