// Command assistant is the single binary that replaces the Plex Scanner and
// Transcoder inside the container to provide native STRM support.
package main

import (
	"os"

	"github.com/PlexAssistant/PlexAssistant/internal/cli"
)

func main() {
	if err := cli.Execute(); err != nil {
		os.Exit(1)
	}
}
