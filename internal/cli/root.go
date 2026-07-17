// Package cli wires the assistant command tree.
//
// The binary is multi-call: when Plex invokes it through the install-time
// symlinks (Scanner / Transcoder), the subcommand is derived from argv[0];
// manual invocations use subcommands directly.
package cli

import (
	"os"
	"path/filepath"
	"strings"

	"github.com/spf13/cobra"
)

func newRoot() *cobra.Command {
	root := &cobra.Command{
		Use:   "assistant",
		Short: "Native STRM support for Plex",
	}
	root.AddCommand(
		newScannerCmd(),
		newTranscoderCmd(),
		newRuntimeCmd(),
		newInstallCmd(),
	)
	return root
}

// dispatchArgv0 maps argv[0] to a subcommand by basename. Exact symlink
// names are confirmed against a real Plex install.
func dispatchArgv0(argv0 string) (string, bool) {
	base := strings.ToLower(filepath.Base(argv0))
	switch {
	case strings.Contains(base, "scanner"):
		return "scanner", true
	case strings.Contains(base, "transcoder"):
		return "transcoder", true
	default:
		return "", false
	}
}

// Execute resolves the invocation mode and runs the matching command.
func Execute() error {
	root := newRoot()
	args := os.Args[1:]
	if sub, ok := dispatchArgv0(os.Args[0]); ok && !hasCommand(root, args) {
		args = append([]string{sub}, args...)
	}
	root.SetArgs(args)
	return root.Execute()
}

func hasCommand(root *cobra.Command, args []string) bool {
	if len(args) == 0 {
		return false
	}
	for _, c := range root.Commands() {
		if c.Name() == args[0] {
			return true
		}
	}
	return false
}
