package transcoder

import (
	"context"
	"os"
	"path/filepath"
	"testing"
)

type fakeResolver struct {
	links    map[string]string // target -> direct
	refresh  map[string]string // target -> refreshed link
	resolves int
}

func (f *fakeResolver) Resolve(_ context.Context, target string) (string, error) {
	f.resolves++
	if d, ok := f.links[target]; ok {
		return d, nil
	}
	return "", nil
}

func (f *fakeResolver) Refresh(_ context.Context, target string) (string, error) {
	if d, ok := f.refresh[target]; ok {
		return d, nil
	}
	return "", nil
}

func writeSTRM(t *testing.T, path, url string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(url+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestRewriteArgs(t *testing.T) {
	dir := t.TempDir()
	strmA := filepath.Join(dir, "a.strm")
	strmB := filepath.Join(dir, "b.strm")
	local := filepath.Join(dir, "c.mkv")
	writeSTRM(t, strmA, "http://x/file/ep1")
	writeSTRM(t, strmB, "/local/path.mkv") // local target: keep
	if err := os.WriteFile(local, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}

	res := &fakeResolver{links: map[string]string{"http://x/file/ep1": "http://x/dl/fresh"}}
	args := []string{"-codec:0", "h264", "-i", strmA, "-i", strmB, "-i", local}
	out, swaps := RewriteArgs(context.Background(), args, res, "UA/1")

	if len(swaps) != 1 {
		t.Fatalf("swaps = %+v", swaps)
	}
	// swapped input is preceded by the resolving user agent
	want := []string{"-codec:0", "h264", "-user_agent", "UA/1", "-i", "http://x/dl/fresh", "-i", strmB, "-i", local}
	if len(out) != len(want) {
		t.Fatalf("out = %v", out)
	}
	for i := range want {
		if out[i] != want[i] {
			t.Fatalf("out[%d] = %s, want %s (full: %v)", i, out[i], want[i], out)
		}
	}
	if swaps[0].ArgIndex != 5 {
		t.Fatalf("swap index = %d", swaps[0].ArgIndex)
	}
}

// fake original binary: fails when its input arg contains DEADLINK,
// succeeds otherwise.
func fakeReal(t *testing.T) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "real")
	script := "#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in *DEADLINK*) exit 9;; esac; done\nexit 0\n"
	if err := os.WriteFile(path, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	return path
}

func TestRunRetriesOnDeadLink(t *testing.T) {
	dir := t.TempDir()
	strm := filepath.Join(dir, "a.strm")
	writeSTRM(t, strm, "http://x/file/ep1")

	// first resolve returns a dead link, refresh returns a good one
	res := &fakeResolver{
		links:   map[string]string{"http://x/file/ep1": "http://DEADLINK/old"},
		refresh: map[string]string{"http://x/file/ep1": "http://x/dl/good"},
	}

	code := Run([]string{"-i", strm}, fakeReal(t), res, "UA/1")
	if code != 0 {
		t.Fatalf("exit = %d, want 0 after refresh retry", code)
	}
	if res.resolves != 1 {
		t.Fatalf("resolve calls = %d", res.resolves)
	}
}

func TestRunNoSwapsNoRetry(t *testing.T) {
	dir := t.TempDir()
	local := filepath.Join(dir, "c.mkv")
	if err := os.WriteFile(local, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	// fake binary still fails on DEADLINK, but no swap means no retry
	code := Run([]string{"-i", local, "DEADLINK"}, fakeReal(t), &fakeResolver{}, "UA/1")
	if code != 9 {
		t.Fatalf("exit = %d, want 9 (no retry without swap)", code)
	}
}
