package main

import (
	"bytes"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"
)

const testBody = "0123456789abcdef"

func newTestServer(t *testing.T, defTTL time.Duration) *httptest.Server {
	t.Helper()
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "ep1.mkv"), []byte(testBody), 0o644); err != nil {
		t.Fatal(err)
	}
	ts := httptest.NewServer(newServer("k", root, defTTL).mux())
	t.Cleanup(ts.Close)
	return ts
}

func ctl(t *testing.T, ts *httptest.Server, body string) {
	t.Helper()
	resp, err := http.Post(ts.URL+"/ctl", "application/json", bytes.NewBufferString(body))
	if err != nil {
		t.Fatal(err)
	}
	_ = resp.Body.Close()
	if resp.StatusCode != http.StatusNoContent {
		t.Fatalf("ctl %q: status %d", body, resp.StatusCode)
	}
}

func TestResolveAndDownload(t *testing.T) {
	ts := newTestServer(t, time.Hour)

	resp, err := http.Get(ts.URL + "/file/ep1.mkv")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("status %d after redirect", resp.StatusCode)
	}
	body, _ := io.ReadAll(resp.Body)
	if string(body) != testBody {
		t.Fatalf("body %q, want %q", body, testBody)
	}
}

func TestInvalidToken(t *testing.T) {
	ts := newTestServer(t, time.Hour)

	resp, err := http.Get(ts.URL + "/dl/ep1.mkv?t=99999999.deadbeefdeadbeef")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("status %d, want 403", resp.StatusCode)
	}
}

func TestExpiredLink(t *testing.T) {
	ts := newTestServer(t, -time.Minute)

	resp, err := http.Get(ts.URL + "/file/ep1.mkv")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusGone {
		t.Fatalf("status %d after redirect, want 410", resp.StatusCode)
	}
}

func TestForbiddenBehavior(t *testing.T) {
	ts := newTestServer(t, time.Hour)
	ctl(t, ts, `{"id":"ep1.mkv","behavior":"forbidden"}`)

	resp, err := http.Get(ts.URL + "/file/ep1.mkv")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("status %d after redirect, want 403", resp.StatusCode)
	}
}

func TestCtlReset(t *testing.T) {
	ts := newTestServer(t, time.Hour)
	ctl(t, ts, `{"id":"ep1.mkv","behavior":"forbidden"}`)
	ctl(t, ts, `{"id":"*","behavior":"normal"}`)

	resp, err := http.Get(ts.URL + "/file/ep1.mkv")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("status %d after reset, want 200", resp.StatusCode)
	}
}

func TestMissingFile(t *testing.T) {
	ts := newTestServer(t, time.Hour)

	resp, err := http.Get(ts.URL + "/file/missing.mkv")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("status %d, want 404", resp.StatusCode)
	}
}
