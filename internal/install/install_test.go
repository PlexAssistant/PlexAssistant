package install

import (
	"os"
	"path/filepath"
	"testing"
)

// newFixture creates a fake plex dir with one target binary and a fake
// assistant binary, returning the installer and both paths.
func newFixture(t *testing.T) (*Installer, string) {
	t.Helper()
	plexDir := t.TempDir()
	binDir := t.TempDir()

	target := filepath.Join(plexDir, "Plex Media Scanner")
	if err := os.WriteFile(target, []byte("plex original"), 0o755); err != nil {
		t.Fatal(err)
	}
	bin := filepath.Join(binDir, "assistant")
	if err := os.WriteFile(bin, []byte("assistant bytes"), 0o755); err != nil {
		t.Fatal(err)
	}
	return &Installer{PlexDir: plexDir, BinPath: bin, Version: "test"}, target
}

func TestInstallVerifyRevert(t *testing.T) {
	inst, target := newFixture(t)

	if err := inst.Do(); err != nil {
		t.Fatalf("install: %v", err)
	}

	// target became a symlink to our binary, original preserved
	link, err := os.Readlink(target)
	if err != nil {
		t.Fatalf("target not a symlink: %v", err)
	}
	if link != inst.BinPath {
		t.Fatalf("symlink points to %s", link)
	}
	orig, err := os.ReadFile(target + ".real")
	if err != nil || string(orig) != "plex original" {
		t.Fatalf("original not preserved: %q %v", orig, err)
	}

	for _, r := range inst.Verify() {
		if !r.OK {
			t.Fatalf("verify failed: %+v", r)
		}
	}
	s, err := inst.State()
	if err != nil || s.Version != "test" {
		t.Fatalf("state: %+v %v", s, err)
	}

	if err := inst.Revert(); err != nil {
		t.Fatalf("revert: %v", err)
	}
	got, err := os.ReadFile(target)
	if err != nil || string(got) != "plex original" {
		t.Fatalf("revert did not restore original: %q %v", got, err)
	}
	if _, err := os.Stat(filepath.Join(inst.PlexDir, stateFile)); !os.IsNotExist(err) {
		t.Fatal("state file not removed on revert")
	}
}

func TestInstallIdempotent(t *testing.T) {
	inst, target := newFixture(t)

	if err := inst.Do(); err != nil {
		t.Fatalf("first install: %v", err)
	}
	if err := os.WriteFile(target+".real", []byte("plex original v2"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := inst.Do(); err != nil {
		t.Fatalf("second install: %v", err)
	}
	orig, err := os.ReadFile(target + ".real")
	if err != nil || string(orig) != "plex original v2" {
		t.Fatalf("re-install clobbered the .real original: %q %v", orig, err)
	}
}

func TestInstallMissingTarget(t *testing.T) {
	inst, _ := newFixture(t)
	if err := os.Remove(filepath.Join(inst.PlexDir, "Plex Media Scanner")); err != nil {
		t.Fatal(err)
	}
	if err := inst.Do(); err == nil {
		t.Fatal("expected error for missing target")
	}
}

func TestRevertRefusesNonSymlink(t *testing.T) {
	inst, target := newFixture(t)
	if err := inst.Do(); err != nil {
		t.Fatal(err)
	}
	// sneak a real file over our symlink
	if err := os.Remove(target); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(target, []byte("important"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := inst.Revert(); err == nil {
		t.Fatal("expected revert to refuse removing a non-symlink")
	}
}
