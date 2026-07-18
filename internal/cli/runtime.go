package cli

import (
	"fmt"
	"path/filepath"

	"github.com/spf13/cobra"

	"github.com/PlexAssistant/PlexAssistant/internal/linkres"
	"github.com/PlexAssistant/PlexAssistant/internal/runtime"
)

// newRuntimeCmd runs the resident link-cache daemon. Links are stored
// persistently and trusted until playback reports them dead.
func newRuntimeCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "runtime",
		Short: "Run the resident link-cache daemon",
		RunE: func(cmd *cobra.Command, _ []string) error {
			sock, _ := cmd.Flags().GetString("sock")
			cachePath, _ := cmd.Flags().GetString("cache")
			ua, _ := cmd.Flags().GetString("ua")

			cache, err := runtime.OpenCache(cachePath)
			if err != nil {
				return err
			}
			defer func() { _ = cache.Close() }()

			quota, _ := cmd.Flags().GetInt("quota-per-hour")
			fmt.Printf("runtime: listening on %s, cache %s, quota %d/h\n", sock, cachePath, quota)
			return runtime.NewDaemon(cache, ua, quota).Serve(sock)
		},
	}
	cmd.Flags().String("sock", runtime.DefaultSock, "unix socket path")
	cmd.Flags().String("cache",
		filepath.Join("/config/assistant", "cache.db"), "cache database path")
	cmd.Flags().String("ua", linkres.DefaultUA, "user agent for cloud drives")
	cmd.Flags().Int("quota-per-hour", 30, "scan resolutions per upstream domain per hour")
	return cmd
}
