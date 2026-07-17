// Package install replaces Plex's scanner binary with a symlink to this
// binary and records the installation state for verify and revert.
package install

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"time"
)

// Targets are the Plex binaries we take over; the scanner is the analysis
// hook ("--analyze --item N"), the transcoder join later.
var Targets = []string{"Plex Media Scanner"}

const stateFile = "assistant-install.json"

// State describes a completed install; it lives next to the targets so it
// disappears together with the container layer.
type State struct {
	Version     string    `json:"version"`
	Binary      string    `json:"binary"`
	SHA256      string    `json:"sha256"`
	Targets     []string  `json:"targets"`
	InstalledAt time.Time `json:"installed_at"`
}

// Installer replaces target binaries inside one Plex directory.
type Installer struct {
	PlexDir string // directory containing the target binaries
	BinPath string // absolute path to this binary
	Version string
}

// Do installs all targets. It is idempotent: re-running refreshes the
// symlinks without touching the preserved ".real" originals.
func (i *Installer) Do() error {
	for _, name := range Targets {
		target := filepath.Join(i.PlexDir, name)
		realPath := target + ".real"

		_, targetErr := os.Lstat(target)
		_, realErr := os.Lstat(realPath)
		if errors.Is(targetErr, os.ErrNotExist) && errors.Is(realErr, os.ErrNotExist) {
			return fmt.Errorf("target not found: %s", target)
		}
		if errors.Is(realErr, os.ErrNotExist) {
			if err := os.Rename(target, realPath); err != nil {
				return fmt.Errorf("preserve original %s: %w", name, err)
			}
		}

		// Replace any previous symlink (or leftover) with one to us.
		if err := os.Remove(target); err != nil && !errors.Is(err, os.ErrNotExist) {
			return fmt.Errorf("clear target %s: %w", name, err)
		}
		if err := os.Symlink(i.BinPath, target); err != nil {
			return fmt.Errorf("link %s: %w", name, err)
		}
	}

	sum, err := fileSum(i.BinPath)
	if err != nil {
		return fmt.Errorf("hash binary: %w", err)
	}
	state := &State{
		Version:     i.Version,
		Binary:      i.BinPath,
		SHA256:      sum,
		Targets:     Targets,
		InstalledAt: time.Now().UTC(),
	}
	return i.writeState(state)
}

// Result is one target's verification outcome.
type Result struct {
	Target  string
	OK      bool
	Problem string
}

// Verify checks that every target is our symlink and the original is kept.
func (i *Installer) Verify() []Result {
	var out []Result
	for _, name := range Targets {
		r := Result{Target: name, OK: true}
		target := filepath.Join(i.PlexDir, name)
		link, err := os.Readlink(target)
		switch {
		case errors.Is(err, os.ErrNotExist):
			r.OK, r.Problem = false, "not installed"
		case err != nil:
			r.OK, r.Problem = false, fmt.Sprintf("not a symlink: %v", err)
		case link != i.BinPath:
			r.OK, r.Problem = false, fmt.Sprintf("points to %s", link)
		}
		if _, err := os.Stat(target + ".real"); errors.Is(err, os.ErrNotExist) {
			r.OK, r.Problem = false, "missing .real original"
		}
		out = append(out, r)
	}
	return out
}

// Revert restores the original binaries. It only removes targets that are
// symlinks, so a misdirected install can never delete a real Plex file.
func (i *Installer) Revert() error {
	for _, name := range Targets {
		target := filepath.Join(i.PlexDir, name)
		realPath := target + ".real"

		info, err := os.Lstat(target)
		if err == nil && info.Mode()&os.ModeSymlink == 0 {
			return fmt.Errorf("refusing to remove non-symlink %s", target)
		}
		if _, err := os.Lstat(realPath); err != nil {
			continue // nothing to revert for this target
		}
		if err := os.Remove(target); err != nil && !errors.Is(err, os.ErrNotExist) {
			return fmt.Errorf("remove symlink %s: %w", name, err)
		}
		if err := os.Rename(realPath, target); err != nil {
			return fmt.Errorf("restore original %s: %w", name, err)
		}
	}
	err := os.Remove(filepath.Join(i.PlexDir, stateFile))
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return fmt.Errorf("remove state: %w", err)
	}
	return nil
}

// State returns the recorded install state, if any.
func (i *Installer) State() (*State, error) {
	b, err := os.ReadFile(filepath.Join(i.PlexDir, stateFile))
	if err != nil {
		return nil, err
	}
	var s State
	if err := json.Unmarshal(b, &s); err != nil {
		return nil, err
	}
	return &s, nil
}

func (i *Installer) writeState(s *State) error {
	b, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(i.PlexDir, stateFile), b, 0o644)
}

func fileSum(path string) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer func() { _ = f.Close() }()
	h := sha256.New()
	if _, err := io.Copy(h, f); err != nil {
		return "", err
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}
