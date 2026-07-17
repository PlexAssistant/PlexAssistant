package runtime

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strconv"
	"testing"
	"time"
)

func newTestCache(t *testing.T) *Cache {
	t.Helper()
	cache, err := OpenCache(filepath.Join(t.TempDir(), "cache.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = cache.Close() })
	return cache
}

func TestCacheRoundtrip(t *testing.T) {
	cache := newTestCache(t)

	if e, err := cache.Get("http://x/file/ep1"); err != nil || e != nil {
		t.Fatalf("get on empty: %v %v", e, err)
	}
	if err := cache.Put(&Entry{
		Target: "http://x/file/ep1", Direct: "http://x/dl/ep1?t=123",
		ExpiresAt: 1799999999,
	}); err != nil {
		t.Fatal(err)
	}
	e, err := cache.Get("http://x/file/ep1")
	if err != nil || e == nil {
		t.Fatalf("get: %v %v", e, err)
	}
	if e.Direct != "http://x/dl/ep1?t=123" || e.ExpiresAt != 1799999999 || e.Hits != 1 {
		t.Fatalf("entry = %+v", e)
	}
	// upsert replaces the link
	if err := cache.Put(&Entry{
		Target: "http://x/file/ep1", Direct: "http://x/dl/ep1?t=456",
	}); err != nil {
		t.Fatal(err)
	}
	e, err = cache.Get("http://x/file/ep1")
	if err != nil || e.Direct != "http://x/dl/ep1?t=456" {
		t.Fatalf("after upsert: %+v %v", e, err)
	}
}

// A stored link stays trusted regardless of its stored expiry; no
// upstream request is ever made for a cache hit.
func TestDaemonResolveTrustsCache(t *testing.T) {
	cache := newTestCache(t)
	target := "http://x/file/ep1"
	stale := "http://x/dl/ep1?t=stale"
	if err := cache.Put(&Entry{Target: target, Direct: stale, ExpiresAt: 1}); err != nil {
		t.Fatal(err)
	}

	d := NewDaemon(cache, "")
	ts := httptest.NewServer(d.Handler())
	t.Cleanup(ts.Close)

	got := resolveVia(t, ts.URL, target)
	if !got.Cached || got.Direct != stale {
		t.Fatalf("stale link must be served as-is: %+v", got)
	}
}

func TestDaemonResolveAndCacheHit(t *testing.T) {
	cache := newTestCache(t)

	exp := time.Now().Add(time.Hour).Unix()
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, "/dl/ep1?t="+strconv.FormatInt(exp, 10)+".sig", http.StatusFound)
	}))
	t.Cleanup(upstream.Close)

	d := NewDaemon(cache, "")
	ts := httptest.NewServer(d.Handler())
	t.Cleanup(ts.Close)

	target := upstream.URL + "/file/ep1"

	got := resolveVia(t, ts.URL, target)
	if got.Cached {
		t.Fatal("first resolve must not be cached")
	}
	if got.ExpiresAt != exp {
		t.Fatalf("sniffed expiry = %d, want %d", got.ExpiresAt, exp)
	}

	again := resolveVia(t, ts.URL, target)
	if !again.Cached || again.Direct != got.Direct {
		t.Fatalf("second resolve = %+v, want cached %s", again, got.Direct)
	}

	stats, err := cache.Stats()
	if err != nil || stats.Entries != 1 || stats.Hits < 1 {
		t.Fatalf("stats = %+v err=%v", stats, err)
	}
}

func resolveVia(t *testing.T, base, target string) *Resolved {
	t.Helper()
	resp, err := http.Post(base+"/resolve", "application/json",
		bytes.NewBufferString(`{"target":"`+target+`"}`))
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("resolve status %d", resp.StatusCode)
	}
	var out Resolved
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		t.Fatal(err)
	}
	return &out
}
