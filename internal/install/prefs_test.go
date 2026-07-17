package install

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestSetPrefs(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "Preferences.xml")
	if err := os.WriteFile(path, []byte(`<Preferences FriendlyName="rig" LogLevel="debug"/>`), 0o644); err != nil {
		t.Fatal(err)
	}

	if err := setPrefs(dir); err != nil {
		t.Fatal(err)
	}
	raw, _ := os.ReadFile(path)
	s := string(raw)
	for _, k := range []string{"FSEventLibraryUpdatesEnabled", "FSEventLibraryPartialScanEnabled"} {
		if !strings.Contains(s, k+`="1"`) {
			t.Fatalf("missing %s in %s", k, s)
		}
	}
	// existing attributes preserved
	if !strings.Contains(s, `FriendlyName="rig"`) || !strings.Contains(s, `LogLevel="debug"`) {
		t.Fatalf("other attrs lost: %s", s)
	}

	// idempotent second run changes nothing
	before := s
	if err := setPrefs(dir); err != nil {
		t.Fatal(err)
	}
	after, _ := os.ReadFile(path)
	if string(after) != before {
		t.Fatalf("second run rewrote file:\n%s\n%s", before, after)
	}
}

func TestSetPrefsOverwritesWrongValue(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "Preferences.xml")
	if err := os.WriteFile(path, []byte(`<Preferences FSEventLibraryUpdatesEnabled="0"/>`), 0o644); err != nil {
		t.Fatal(err)
	}

	if err := setPrefs(dir); err != nil {
		t.Fatal(err)
	}
	raw, _ := os.ReadFile(path)
	if !strings.Contains(string(raw), `FSEventLibraryUpdatesEnabled="1"`) {
		t.Fatalf("value not corrected: %s", raw)
	}
}

func TestSetPrefsMissingFile(t *testing.T) {
	if err := setPrefs(t.TempDir()); err != nil {
		t.Fatalf("missing file must be a no-op: %v", err)
	}
}
