package linkres

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strconv"
	"testing"
	"time"
)

func TestParseSTRM(t *testing.T) {
	dir := t.TempDir()
	write := func(name, content string) string {
		p := filepath.Join(dir, name)
		if err := os.WriteFile(p, []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
		return p
	}

	tests := []struct {
		name, content, want string
		remote, wantErr     bool
	}{
		{"plain", "http://x/f.mkv\n", "http://x/f.mkv", true, false},
		{"bom and comments", "\xEF\xBB\xBF# comment\n\n  http://x/f.mkv  \n", "http://x/f.mkv", true, false},
		{"local path", "/media/movies/a.mkv", "/media/movies/a.mkv", false, false},
		{"empty", "# only comments\n\n", "", false, true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := ParseSTRM(write(tt.name+".strm", tt.content))
			if tt.wantErr {
				if err == nil {
					t.Fatalf("expected error, got %+v", got)
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			if got.Value != tt.want || got.IsRemote != tt.remote {
				t.Fatalf("got %+v", got)
			}
		})
	}
}

// newCloud simulates a cloud drive: /file redirects to /dl, which serves
// bytes with range support; behavior endpoints answer the failure codes.
func newCloud(t *testing.T, capturedUA *string) *httptest.Server {
	t.Helper()
	mux := http.NewServeMux()
	mux.HandleFunc("GET /file/{id}", func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, "/dl/"+r.PathValue("id"), http.StatusFound)
	})
	mux.HandleFunc("GET /dl/{id}", func(w http.ResponseWriter, r *http.Request) {
		if capturedUA != nil {
			*capturedUA = r.Header.Get("User-Agent")
		}
		switch r.PathValue("id") {
		case "forbidden":
			http.Error(w, "forbidden", http.StatusForbidden)
			return
		case "gone":
			http.Error(w, "gone", http.StatusGone)
			return
		case "missing":
			http.Error(w, "not found", http.StatusNotFound)
			return
		}
		body := "0123456789"
		if rng := r.Header.Get("Range"); rng != "" {
			w.Header().Set("Content-Range", "bytes 0-0/"+strconv.Itoa(len(body)))
			w.WriteHeader(http.StatusPartialContent)
			_, _ = w.Write([]byte(body[:1]))
			return
		}
		_, _ = w.Write([]byte(body))
	})
	ts := httptest.NewServer(mux)
	t.Cleanup(ts.Close)
	return ts
}

func TestResolve(t *testing.T) {
	var ua string
	ts := newCloud(t, &ua)
	ctx := context.Background()

	link, err := Resolve(ctx, ts.URL+"/file/ep1.mkv", DefaultUA)
	if err != nil {
		t.Fatal(err)
	}
	if link.URL != ts.URL+"/dl/ep1.mkv" {
		t.Fatalf("final url %s", link.URL)
	}
	if len(link.Chain) != 1 || link.Chain[0] != ts.URL+"/dl/ep1.mkv" {
		t.Fatalf("chain %v", link.Chain)
	}
	if !link.RangeSupported || link.Size != 10 {
		t.Fatalf("link = %+v", link)
	}
	if ua != DefaultUA {
		t.Fatalf("ua sent = %q", ua)
	}
}

func TestSniffExpiry(t *testing.T) {
	now := time.Now()
	future := now.Add(time.Hour).Unix()
	past := now.Add(-time.Hour).Unix()

	tests := []struct {
		name, url string
		want      int64
	}{
		{"signed token", "http://x/dl/a?t=" + itoa(future) + ".deadbeef", future},
		{"plain exp", "http://x/dl/a?exp=" + itoa(future), future},
		{"past ignored", "http://x/dl/a?t=" + itoa(past), 0},
		{"no timestamp", "http://x/dl/a?sig=abcdef1234", 0},
		{"wrong length", "http://x/dl/a?t=" + itoa(future) + "9", 0},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := SniffExpiry(tt.url, now); got != tt.want {
				t.Fatalf("SniffExpiry(%q) = %d, want %d", tt.url, got, tt.want)
			}
		})
	}
}

func itoa(n int64) string { return strconv.FormatInt(n, 10) }

func TestResolveFailures(t *testing.T) {
	ts := newCloud(t, nil)
	ctx := context.Background()

	tests := []struct {
		id   string
		want error
	}{
		{"missing", ErrDead},
		{"forbidden", ErrForbidden},
		{"gone", ErrGone},
	}
	for _, tt := range tests {
		t.Run(tt.id, func(t *testing.T) {
			if _, err := Resolve(ctx, ts.URL+"/file/"+tt.id, ""); !errors.Is(err, tt.want) {
				t.Fatalf("err = %v, want %v", err, tt.want)
			}
		})
	}
}
