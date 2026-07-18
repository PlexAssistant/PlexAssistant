// Package runtime is the resident daemon: a persistent direct-link
// cache served over UDS. Links are trusted until a playback failure
// reports them dead; expiry timestamps are stored for information only.
package runtime

import (
	"database/sql"
	"os"
	"path/filepath"
	"time"

	_ "modernc.org/sqlite" // registers the "sqlite" driver
)

const schema = `
CREATE TABLE IF NOT EXISTS links (
	target_url  TEXT PRIMARY KEY,
	direct_url  TEXT NOT NULL,
	ua          TEXT,
	expires_at  INTEGER,
	cached_at   INTEGER NOT NULL,
	last_used   INTEGER NOT NULL,
	hits        INTEGER NOT NULL DEFAULT 0
)`

// Entry is one cached direct link.
type Entry struct {
	Target    string
	Direct    string
	UA        string
	ExpiresAt int64 // unix seconds, 0 = unknown (informational)
	CachedAt  int64
	LastUsed  int64
	Hits      int64
}

// Cache is the persistent link store.
type Cache struct {
	db *sql.DB
}

// OpenCache opens (creating if needed) the cache database at path.
func OpenCache(path string) (*Cache, error) {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return nil, err
	}
	db, err := sql.Open("sqlite", "file:"+path+"?_pragma=busy_timeout(5000)")
	if err != nil {
		return nil, err
	}
	if _, err := db.Exec(schema); err != nil {
		_ = db.Close()
		return nil, err
	}
	if _, err := db.Exec(failuresSchema); err != nil {
		_ = db.Close()
		return nil, err
	}
	return &Cache{db: db}, nil
}

// Close closes the cache database.
func (c *Cache) Close() error {
	return c.db.Close()
}

// Get returns the cached entry, bumping its use counters, or nil.
func (c *Cache) Get(target string) (*Entry, error) {
	row := c.db.QueryRow(
		`SELECT target_url, direct_url, IFNULL(ua,''), IFNULL(expires_at,0),
		        cached_at, last_used, hits FROM links WHERE target_url=?`, target)
	var e Entry
	err := row.Scan(&e.Target, &e.Direct, &e.UA, &e.ExpiresAt, &e.CachedAt, &e.LastUsed, &e.Hits)
	if err == sql.ErrNoRows {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	if _, err := c.db.Exec(
		`UPDATE links SET hits=hits+1, last_used=? WHERE target_url=?`,
		now(), target); err != nil {
		return nil, err
	}
	e.Hits++
	e.LastUsed = now()
	return &e, nil
}

// Put upserts an entry.
func (c *Cache) Put(e *Entry) error {
	ts := now()
	if e.CachedAt == 0 {
		e.CachedAt = ts
	}
	e.LastUsed = ts
	_, err := c.db.Exec(`
		INSERT INTO links (target_url, direct_url, ua, expires_at, cached_at, last_used, hits)
		VALUES (?,?,?,?,?,?,0)
		ON CONFLICT(target_url) DO UPDATE SET
			direct_url=excluded.direct_url, ua=excluded.ua,
			expires_at=excluded.expires_at, last_used=excluded.last_used`,
		e.Target, e.Direct, e.UA, e.ExpiresAt, e.CachedAt, e.LastUsed)
	return err
}

// Stats summarizes the cache for /state.
type Stats struct {
	Entries int64
	Hits    int64
}

// Stats reports cache totals.
func (c *Cache) Stats() (Stats, error) {
	var s Stats
	err := c.db.QueryRow(`SELECT count(*), IFNULL(sum(hits),0) FROM links`).
		Scan(&s.Entries, &s.Hits)
	return s, err
}

func now() int64 { return time.Now().Unix() }
