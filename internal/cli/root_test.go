package cli

import "testing"

func TestDispatchArgv0(t *testing.T) {
	tests := []struct {
		argv0 string
		want  string
		ok    bool
	}{
		{"/config/Library/Application Support/Plex Media Server/Scanners/Movies/Plex Movie Scanner", "scanner", true},
		{"/usr/lib/plexmediaserver/Plex Transcoder.native", "transcoder", true},
		{"assistant", "", false},
		{"/usr/local/bin/assistant", "", false},
	}
	for _, tt := range tests {
		got, ok := dispatchArgv0(tt.argv0)
		if got != tt.want || ok != tt.ok {
			t.Errorf("dispatchArgv0(%q) = (%q, %v), want (%q, %v)",
				tt.argv0, got, ok, tt.want, tt.ok)
		}
	}
}

func TestHasCommand(t *testing.T) {
	root := newRoot()
	for _, name := range []string{"scanner", "transcoder", "runtime", "install"} {
		if !hasCommand(root, []string{name}) {
			t.Errorf("hasCommand(%q) = false, want true", name)
		}
	}
	if hasCommand(root, []string{"nope"}) {
		t.Error(`hasCommand("nope") = true, want false`)
	}
	if hasCommand(root, nil) {
		t.Error("hasCommand(nil) = true, want false")
	}
}
