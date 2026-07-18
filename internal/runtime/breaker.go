package runtime

import (
	"sync"
	"time"
)

// Breaker states.
const (
	closed = iota
	open
	halfOpen
)

// breakerConfig bounds the failure response.
type breakerConfig struct {
	threshold int           // consecutive failures before opening
	baseCool  time.Duration // first cooldown
	maxCool   time.Duration // cooldown ceiling
}

var defaultBreaker = breakerConfig{threshold: 3, baseCool: 3 * time.Minute, maxCool: 30 * time.Minute}

// breaker guards one upstream domain: closed passes everything, open
// rejects instantly, half-open admits exactly one probe.
type breaker struct {
	mu      sync.Mutex
	cfg     breakerConfig
	state   int
	fails   int
	openUnt time.Time
	cool    time.Duration
	claimed bool // half-open probe in flight
}

// Allow reports whether a request may pass now. force (playback)
// always passes and does not claim the probe slot.
func (b *breaker) Allow(now time.Time, force bool) (allowed, probe bool) {
	b.mu.Lock()
	defer b.mu.Unlock()

	if force {
		return true, false
	}
	switch b.state {
	case closed:
		return true, false
	case open:
		if now.Before(b.openUnt) {
			return false, false
		}
		b.state = halfOpen
		b.claimed = true
		return true, true
	default: // halfOpen
		if b.claimed {
			return false, false
		}
		b.claimed = true
		return true, true
	}
}

// RecordSuccess closes the breaker and resets the backoff sequence.
func (b *breaker) RecordSuccess() {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.state = closed
	b.fails = 0
	b.cool = 0
	b.claimed = false
}

// RecordFailure counts a failure and opens the breaker once the
// threshold is hit, doubling the cooldown each consecutive open.
func (b *breaker) RecordFailure(now time.Time) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.fails++
	b.claimed = false
	if b.state == halfOpen || b.fails >= b.cfg.threshold {
		if b.cool < b.cfg.baseCool {
			b.cool = b.cfg.baseCool
		} else if b.cool < b.cfg.maxCool {
			b.cool *= 2
			if b.cool > b.cfg.maxCool {
				b.cool = b.cfg.maxCool
			}
		}
		b.state = open
		b.openUnt = now.Add(b.cool)
	}
}

// State names the breaker state for /state.
func (b *breaker) State() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	switch b.state {
	case open:
		return "open"
	case halfOpen:
		return "half-open"
	default:
		return "closed"
	}
}

// breakerSet keeps one breaker per upstream domain.
type breakerSet struct {
	mu       sync.Mutex
	breakers map[string]*breaker
}

func newBreakerSet() *breakerSet {
	return &breakerSet{breakers: map[string]*breaker{}}
}

func (s *breakerSet) get(domain string) *breaker {
	s.mu.Lock()
	defer s.mu.Unlock()
	b, ok := s.breakers[domain]
	if !ok {
		b = &breaker{cfg: defaultBreaker, cool: 0}
		s.breakers[domain] = b
	}
	return b
}

// snapshot returns breaker states keyed by domain.
func (s *breakerSet) snapshot() map[string]string {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make(map[string]string, len(s.breakers))
	for d, b := range s.breakers {
		out[d] = b.State()
	}
	return out
}
