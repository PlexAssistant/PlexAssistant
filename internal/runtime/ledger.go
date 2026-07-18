package runtime

import (
	"database/sql"
	"time"
)

// Dead-link backoff sequence: first failure retries after a day, then
// two, four, capped at a week.
var backoffSteps = []time.Duration{24 * time.Hour, 48 * time.Hour, 96 * time.Hour, 7 * 24 * time.Hour}

const failuresSchema = `
CREATE TABLE IF NOT EXISTS failures (
	target_url TEXT PRIMARY KEY,
	fail_count INTEGER NOT NULL,
	last_fail  INTEGER NOT NULL,
	next_retry INTEGER NOT NULL
)`

// RecordFailure books a failed target with growing backoff.
func (c *Cache) RecordFailure(target string, now time.Time) error {
	var count int
	err := c.db.QueryRow(
		`SELECT fail_count FROM failures WHERE target_url=?`, target).Scan(&count)
	if err != nil && err != sql.ErrNoRows {
		return err
	}
	count++
	step := backoffSteps[len(backoffSteps)-1]
	if count <= len(backoffSteps) {
		step = backoffSteps[count-1]
	}
	_, err = c.db.Exec(`
		INSERT INTO failures (target_url, fail_count, last_fail, next_retry)
		VALUES (?,?,?,?)
		ON CONFLICT(target_url) DO UPDATE SET
			fail_count=excluded.fail_count,
			last_fail=excluded.last_fail,
			next_retry=excluded.next_retry`,
		target, count, now.Unix(), now.Add(step).Unix())
	return err
}

// ClearFailure forgets a target after any success.
func (c *Cache) ClearFailure(target string) error {
	_, err := c.db.Exec(`DELETE FROM failures WHERE target_url=?`, target)
	return err
}

// ShouldSkip reports whether the target is still backing off.
func (c *Cache) ShouldSkip(target string, now time.Time) (bool, error) {
	var next int64
	err := c.db.QueryRow(
		`SELECT next_retry FROM failures WHERE target_url=?`, target).Scan(&next)
	if err == sql.ErrNoRows {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	return next > now.Unix(), nil
}

// FailureCount reports booked failures for /state.
func (c *Cache) FailureCount() (int64, error) {
	var n int64
	err := c.db.QueryRow(`SELECT count(*) FROM failures`).Scan(&n)
	return n, err
}
