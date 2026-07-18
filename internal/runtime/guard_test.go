package runtime

import (
	"path/filepath"
	"testing"
	"time"
)

func TestBreakerOpenAndRecover(t *testing.T) {
	b := &breaker{cfg: defaultBreaker}
	now := time.Now()

	// three consecutive failures open it
	for i := 0; i < 3; i++ {
		if ok, _ := b.Allow(now, false); !ok {
			t.Fatal("must pass while closed")
		}
		b.RecordFailure(now)
	}
	if ok, _ := b.Allow(now.Add(time.Second), false); ok {
		t.Fatal("must reject while open")
	}
	if b.State() != "open" {
		t.Fatalf("state = %s", b.State())
	}

	// after the cooldown exactly one probe passes
	after := now.Add(defaultBreaker.baseCool + time.Second)
	ok, probe := b.Allow(after, false)
	if !ok || !probe {
		t.Fatalf("probe: allowed=%v probe=%v", ok, probe)
	}
	// second request while probe in flight is rejected
	if ok2, _ := b.Allow(after, false); ok2 {
		t.Fatal("second request during probe must be rejected")
	}

	// probe success closes and resets
	b.RecordSuccess()
	if ok, probe := b.Allow(after, false); !ok || probe {
		t.Fatalf("after success: allowed=%v probe=%v", ok, probe)
	}
}

func TestBreakerCooldownDoubles(t *testing.T) {
	b := &breaker{cfg: defaultBreaker}
	now := time.Now()

	// open with 3 failures
	for i := 0; i < 3; i++ {
		b.RecordFailure(now)
	}
	// probe at cooldown boundary fails -> reopen with doubled cooldown
	b.Allow(now.Add(defaultBreaker.baseCool+time.Second), false)
	b.RecordFailure(now.Add(defaultBreaker.baseCool + time.Second))
	if s := b.State(); s != "open" {
		t.Fatalf("state = %s", s)
	}
	// still open past the first cooldown (the doubled cooldown runs from
	// the failed probe: 3m + 6m = 9m from now)
	if ok, _ := b.Allow(now.Add(defaultBreaker.baseCool+time.Second), false); ok {
		t.Fatal("must still be open inside doubled cooldown")
	}
	// open past the doubled cooldown
	ok, _ := b.Allow(now.Add(3*defaultBreaker.baseCool+2*time.Second), false)
	if !ok {
		t.Fatal("probe must pass after doubled cooldown")
	}
}

func TestBreakerForceBypasses(t *testing.T) {
	b := &breaker{cfg: defaultBreaker}
	now := time.Now()
	for i := 0; i < 3; i++ {
		b.RecordFailure(now)
	}
	if ok, _ := b.Allow(now, true); !ok {
		t.Fatal("force must bypass open state")
	}
	// a forced success resets the breaker
	b.RecordSuccess()
	if b.State() != "closed" {
		t.Fatal("forced success must close the breaker")
	}
}

func TestLedgerBackoff(t *testing.T) {
	cache, err := OpenCache(filepath.Join(t.TempDir(), "cache.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = cache.Close() })

	now := time.Now()
	target := "http://x/file/dead"

	// first failure: retry allowed only after a day
	if err := cache.RecordFailure(target, now); err != nil {
		t.Fatal(err)
	}
	if skip, err := cache.ShouldSkip(target, now.Add(time.Hour)); err != nil || !skip {
		t.Fatalf("skip after 1h: %v %v", skip, err)
	}
	if skip, err := cache.ShouldSkip(target, now.Add(25*time.Hour)); err != nil || skip {
		t.Fatalf("retry after 25h: %v %v", skip, err)
	}

	// repeated failures cap the step at a week
	for i := 0; i < 4; i++ {
		if err := cache.RecordFailure(target, now); err != nil {
			t.Fatal(err)
		}
	}
	if skip, _ := cache.ShouldSkip(target, now.Add(7*24*time.Hour-time.Hour)); !skip {
		t.Fatal("must still skip inside the week")
	}
	if skip, _ := cache.ShouldSkip(target, now.Add(7*24*time.Hour+time.Hour)); skip {
		t.Fatal("retry due after the week")
	}

	// success clears the ledger
	if err := cache.ClearFailure(target); err != nil {
		t.Fatal(err)
	}
	if skip, _ := cache.ShouldSkip(target, now); skip {
		t.Fatal("cleared target must not skip")
	}
}

func TestQuotaWindow(t *testing.T) {
	q := newQuota(3, time.Hour)
	now := time.Now()

	for i := 0; i < 3; i++ {
		if !q.Allow("a.example", now.Add(time.Duration(i)*time.Minute)) {
			t.Fatal("inside limit")
		}
	}
	if q.Allow("a.example", now.Add(3*time.Minute)) {
		t.Fatal("must block over limit")
	}
	// other domains unaffected
	if !q.Allow("b.example", now.Add(3*time.Minute)) {
		t.Fatal("quota is per domain")
	}
	// entries age out of the window
	if !q.Allow("a.example", now.Add(61*time.Minute)) {
		t.Fatal("old hits must expire")
	}
}
