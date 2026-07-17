package plexdb

import (
	"path/filepath"
	"testing"
)

// newFixtureDB builds a temp database with the minimal Plex-like schema
// and one item carrying two parts.
func newFixtureDB(t *testing.T, schemaVersion string) *DB {
	t.Helper()
	db, err := Open(filepath.Join(t.TempDir(), "lib.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = db.Close() })

	stmts := []string{
		`CREATE TABLE schema_migrations (version varchar(255) NOT NULL)`,
		`INSERT INTO schema_migrations VALUES ('` + schemaVersion + `')`,
		`CREATE TABLE media_items (id INTEGER PRIMARY KEY, metadata_item_id INTEGER)`,
		`CREATE TABLE media_parts (id INTEGER PRIMARY KEY, media_item_id INTEGER,
		   file TEXT, size INTEGER DEFAULT 0, duration INTEGER)`,
		`INSERT INTO media_items VALUES (1, 42)`,
		`INSERT INTO media_parts VALUES (10, 1, '/data/a.strm', 35, NULL)`,
		`INSERT INTO media_parts VALUES (11, 1, '/data/b.strm', 48, 1200)`,
	}
	for _, s := range stmts {
		if _, err := db.conn.Exec(s); err != nil {
			t.Fatalf("fixture %q: %v", s, err)
		}
	}
	return db
}

func TestSchemaKnown(t *testing.T) {
	db := newFixtureDB(t, "500000000001.231")
	known, v, err := db.SchemaKnown()
	if err != nil || !known {
		t.Fatalf("known=%v version=%q err=%v", known, v, err)
	}

	db2 := newFixtureDB(t, "999999999999.999")
	known2, v2, err := db2.SchemaKnown()
	if err != nil || known2 {
		t.Fatalf("unknown schema must not be known: %v %q %v", known2, v2, err)
	}
}

func TestPartsByItem(t *testing.T) {
	db := newFixtureDB(t, "500000000001.231")

	parts, err := db.PartsByItem(42)
	if err != nil {
		t.Fatal(err)
	}
	if len(parts) != 2 {
		t.Fatalf("got %d parts, want 2", len(parts))
	}
	if parts[0].File != "/data/a.strm" || parts[0].Size != 35 || parts[0].Duration != 0 {
		t.Fatalf("part[0] = %+v", parts[0])
	}
	if parts[1].Duration != 1200 {
		t.Fatalf("part[1] = %+v", parts[1])
	}

	none, err := db.PartsByItem(999)
	if err != nil || len(none) != 0 {
		t.Fatalf("unknown item: parts=%v err=%v", none, err)
	}
}
