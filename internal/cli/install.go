package cli

import (
	"fmt"
	"os"

	"github.com/spf13/cobra"

	"github.com/PlexAssistant/PlexAssistant/internal/install"
)

// Version is overridden at build time via -ldflags.
var Version = "dev"

func newInstallCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "install",
		Short: "Install scanner/transcoder symlinks into Plex",
	}
	cmd.Flags().String("plex-dir", "/usr/lib/plexmediaserver", "Plex binary directory")
	cmd.Flags().String("support-dir", install.DefaultSupportDir, "Plex data directory")
	cmd.AddCommand(
		newInstallVerifyCmd(),
		newInstallRevertCmd(),
	)
	cmd.RunE = func(cmd *cobra.Command, _ []string) error {
		inst, err := installer(cmd)
		if err != nil {
			return err
		}
		if err := inst.Do(); err != nil {
			return err
		}
		fmt.Printf("installed %v into %s\n", install.Targets, inst.PlexDir)
		return nil
	}
	return cmd
}

func newInstallVerifyCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "verify",
		Short: "Verify installation integrity",
	}
	cmd.Flags().String("plex-dir", "/usr/lib/plexmediaserver", "Plex binary directory")
	cmd.Flags().String("support-dir", install.DefaultSupportDir, "Plex data directory")
	cmd.RunE = func(cmd *cobra.Command, _ []string) error {
		inst, err := installer(cmd)
		if err != nil {
			return err
		}
		failed := false
		for _, r := range inst.Verify() {
			status := "ok"
			if !r.OK {
				status = "FAIL: " + r.Problem
				failed = true
			}
			fmt.Printf("%-20s %s\n", r.Target, status)
		}
		if failed {
			return fmt.Errorf("verification failed")
		}
		return nil
	}
	return cmd
}

func newInstallRevertCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "revert",
		Short: "Restore original Plex binaries",
	}
	cmd.Flags().String("plex-dir", "/usr/lib/plexmediaserver", "Plex binary directory")
	cmd.Flags().String("support-dir", install.DefaultSupportDir, "Plex data directory")
	cmd.RunE = func(cmd *cobra.Command, _ []string) error {
		inst, err := installer(cmd)
		if err != nil {
			return err
		}
		if err := inst.Revert(); err != nil {
			return err
		}
		fmt.Println("originals restored")
		return nil
	}
	return cmd
}

func installer(cmd *cobra.Command) (*install.Installer, error) {
	dir, err := cmd.Flags().GetString("plex-dir")
	if err != nil {
		return nil, err
	}
	support, err := cmd.Flags().GetString("support-dir")
	if err != nil {
		return nil, err
	}
	bin, err := os.Executable()
	if err != nil {
		return nil, err
	}
	return &install.Installer{
		PlexDir:    dir,
		SupportDir: support,
		BinPath:    bin,
		Version:    Version,
	}, nil
}
