package runtime

import (
	"sync"
	"time"
)

// quota limits scan-driven resolutions per upstream domain in a sliding
// window. Playback (force) requests are never counted.
type quota struct {
	mu    sync.Mutex
	limit int
	win   time.Duration
	hits  map[string][]time.Time
}

func newQuota(limit int, window time.Duration) *quota {
	return &quota{limit: limit, win: window, hits: map[string][]time.Time{}}
}

// Allow records and reports whether a scan request may pass.
func (q *quota) Allow(domain string, now time.Time) bool {
	q.mu.Lock()
	defer q.mu.Unlock()

	cutoff := now.Add(-q.win)
	recent := q.hits[domain][:0]
	for _, t := range q.hits[domain] {
		if t.After(cutoff) {
			recent = append(recent, t)
		}
	}
	if len(recent) >= q.limit {
		q.hits[domain] = recent
		return false
	}
	q.hits[domain] = append(recent, now)
	return true
}

// usage reports current window usage per domain for /state.
func (q *quota) usage(now time.Time) map[string]int {
	q.mu.Lock()
	defer q.mu.Unlock()
	cutoff := now.Add(-q.win)
	out := map[string]int{}
	for d, ts := range q.hits {
		n := 0
		for _, t := range ts {
			if t.After(cutoff) {
				n++
			}
		}
		if n > 0 {
			out[d] = n
		}
	}
	return out
}
