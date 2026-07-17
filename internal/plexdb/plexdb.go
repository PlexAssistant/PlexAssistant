// Package plexdb reads and writes the Plex library SQLite database
// directly. The safety protocol (busy_timeout, schema whitelist, backup
// before writes) follows the measured behavior in development-plan §8.
package plexdb

import (
	"database/sql"
	"fmt"
	"time"

	_ "modernc.org/sqlite" // registers the "sqlite" driver
)

// DefaultPath is the library database inside the Plex container.
const DefaultPath = "/config/Library/Application Support/Plex Media Server/Plug-in Support/Databases/com.plexapp.plugins.library.db"

// knownSchemas lists schema_migrations versions this build can handle;
// an unknown version means "refuse to write" (reads stay allowed).
var knownSchemas = map[string]bool{
	"500000000001.231": true, // measured on Plex 1.43.5
}

const busyTimeout = 5 * time.Second

// DB is a handle on one Plex library database.
type DB struct {
	conn *sql.DB
}

// Open connects with the write-safety pragmas applied to every
// connection (SQLite connections are short-lived in database/sql).
func Open(path string) (*DB, error) {
	dsn := fmt.Sprintf("file:%s?_pragma=busy_timeout(%d)", path, busyTimeout.Milliseconds())
	conn, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, err
	}
	if err := conn.Ping(); err != nil {
		_ = conn.Close()
		return nil, err
	}
	return &DB{conn: conn}, nil
}

// Close closes the connection.
func (d *DB) Close() error {
	return d.conn.Close()
}

// SchemaVersion returns schema_migrations.max(version).
func (d *DB) SchemaVersion() (string, error) {
	var v sql.NullString
	if err := d.conn.QueryRow("SELECT max(version) FROM schema_migrations").Scan(&v); err != nil {
		return "", err
	}
	return v.String, nil
}

// SchemaKnown reports whether the database schema is on the whitelist.
func (d *DB) SchemaKnown() (bool, string, error) {
	v, err := d.SchemaVersion()
	if err != nil {
		return false, "", err
	}
	return knownSchemas[v], v, nil
}

// Part is one media part (typically one file) of a metadata item.
type Part struct {
	ID       int64
	File     string
	Size     int64
	Duration int64 // ms, 0 = unknown
}

// PartsByItem returns the media parts of a metadata item, joined through
// media_items.
func (d *DB) PartsByItem(itemID int64) ([]Part, error) {
	rows, err := d.conn.Query(`
		SELECT p.id, p.file, p.size, IFNULL(p.duration, 0)
		FROM media_parts p
		JOIN media_items mi ON mi.id = p.media_item_id
		WHERE mi.metadata_item_id = ?
		ORDER BY p.id`, itemID)
	if err != nil {
		return nil, err
	}
	defer func() { _ = rows.Close() }()

	var out []Part
	for rows.Next() {
		var p Part
		if err := rows.Scan(&p.ID, &p.File, &p.Size, &p.Duration); err != nil {
			return nil, err
		}
		out = append(out, p)
	}
	return out, rows.Err()
}
