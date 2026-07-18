package cli

import (
	"os"

	"github.com/spf13/cobra"

	"github.com/PlexAssistant/PlexAssistant/internal/linkres"
	"github.com/PlexAssistant/PlexAssistant/internal/transcoder"
)

// newTranscoderCmd is the Plex Transcoder wrapper: "-i" strm inputs are
// swapped for cached direct links, then the preserved original runs as
// a child. Flag parsing is disabled: Plex passes raw ffmpeg-style argv.
func newTranscoderCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:                "transcoder",
		Short:              "Run as the Plex Transcoder wrapper",
		DisableFlagParsing: true,
		RunE: func(_ *cobra.Command, args []string) error {
			res := transcoder.DaemonResolver{
				SockPath:  envDefault("ASSISTANT_SOCK", "/run/assistant.sock"),
				UserAgent: linkres.DefaultUA,
			}
			os.Exit(transcoder.Run(args, os.Args[0]+".real", res))
			return nil
		},
	}
	return cmd
}
