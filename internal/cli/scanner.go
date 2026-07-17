package cli

import (
	"fmt"
	"os"
	"strings"
	"syscall"

	"github.com/spf13/cobra"

	"github.com/PlexAssistant/PlexAssistant/internal/linkres"
	"github.com/PlexAssistant/PlexAssistant/internal/plexdb"
	"github.com/PlexAssistant/PlexAssistant/internal/scanner"
)

// newScannerCmd is the Plex Scanner replacement. Item and section
// analyze invocations run our strm pipeline; every other mode (matcher,
// credits, chapter thumbs) is passed through to the preserved original.
func newScannerCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "scanner",
		Short: "Run as the Plex Scanner replacement",
		RunE:  runScanner,
	}
	cmd.Flags().Bool("analyze", false, "analyze mode")
	cmd.Flags().Int("section", 0, "library section id")
	cmd.Flags().Int("item", 0, "metadata item id")
	cmd.Flags().String("library-db", plexdb.DefaultPath, "library database path")
	cmd.Flags().String("ffprobe", envDefault("ASSISTANT_FFPROBE", "ffprobe"), "ffprobe path")
	cmd.Flags().String("ua", linkres.DefaultUA, "user agent for cloud drives")
	cmd.Flags().String("runtime-sock", "/run/assistant.sock",
		"runtime daemon socket; unreachable = resolve directly")
	cmd.FParseErrWhitelist = cobra.FParseErrWhitelist{UnknownFlags: true}
	return cmd
}

func runScanner(cmd *cobra.Command, _ []string) error {
	analyze, _ := cmd.Flags().GetBool("analyze")
	item, _ := cmd.Flags().GetInt("item")
	section, _ := cmd.Flags().GetInt("section")

	if !analyze || (item == 0 && section == 0) {
		return execOriginal(os.Args)
	}

	dbPath, _ := cmd.Flags().GetString("library-db")
	ffprobe, _ := cmd.Flags().GetString("ffprobe")
	ua, _ := cmd.Flags().GetString("ua")
	sock, _ := cmd.Flags().GetString("runtime-sock")
	opts := scanner.Options{FfprobePath: ffprobe, UserAgent: ua, RuntimeSock: sock}

	db, err := plexdb.Open(dbPath)
	if err != nil {
		return err
	}
	defer func() { _ = db.Close() }()

	if item > 0 {
		return analyzeOne(cmd, db, int64(item), opts)
	}
	return analyzeSection(cmd, db, int64(section), opts)
}

func analyzeOne(cmd *cobra.Command, db *plexdb.DB, item int64, opts scanner.Options) error {
	parts, err := db.PartsByItem(item)
	if err != nil {
		return err
	}
	if !anyStrm(parts) {
		return execOriginal(os.Args) // local media: keep the official path
	}
	n, err := scanner.AnalyzeItem(cmd.Context(), db, item, opts)
	if err != nil {
		return err
	}
	fmt.Printf("item %d: analyzed %d strm part(s)\n", item, n)
	return nil
}

func analyzeSection(cmd *cobra.Command, db *plexdb.DB, section int64, opts scanner.Options) error {
	ids, err := db.ItemsBySection(section)
	if err != nil {
		return err
	}
	total := 0
	for _, id := range ids {
		parts, err := db.PartsByItem(id)
		if err != nil {
			return err
		}
		if !anyStrm(parts) {
			continue
		}
		if _, err := scanner.AnalyzeItem(cmd.Context(), db, id, opts); err != nil {
			fmt.Printf("item %d: %v\n", id, err)
		}
		total += len(parts)
	}
	if err := db.Checkpoint(); err != nil {
		fmt.Printf("checkpoint: %v\n", err)
	}
	fmt.Printf("section %d: %d item(s) processed\n", section, len(ids))
	return nil
}

func anyStrm(parts []plexdb.Part) bool {
	for _, p := range parts {
		if strings.HasSuffix(strings.ToLower(p.File), ".strm") {
			return true
		}
	}
	return false
}

// execOriginal replaces this process with the preserved Plex binary so
// unhandled modes keep their official behavior.
func execOriginal(argv []string) error {
	original := argv[0] + ".real"
	if _, err := os.Stat(original); err != nil {
		return fmt.Errorf("original scanner not found at %s", original)
	}
	return syscall.Exec(original, append([]string{original}, argv[1:]...), os.Environ())
}

func envDefault(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}
