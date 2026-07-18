package runtime

import (
	"context"
	"encoding/json"
	"net"
	"net/http"
	"net/url"
	"os"
	"time"

	"github.com/PlexAssistant/PlexAssistant/internal/linkres"
)

// Daemon serves the link cache over a unix socket. Semantics: a link is
// resolved once and then trusted forever; refresh happens only when a
// caller reports it dead while playing. Scan-driven resolutions pass a
// breaker, a dead-link ledger and a per-domain quota; playback (refresh)
// bypasses all three but its outcome still feeds them.
type Daemon struct {
	cache    *Cache
	ua       string
	breakers *breakerSet
	quota    *quota
}

// NewDaemon wires a daemon over cache, resolving with ua and a
// scan-resolution quota of quotaPerHour per upstream domain.
func NewDaemon(cache *Cache, ua string, quotaPerHour int) *Daemon {
	return &Daemon{
		cache:    cache,
		ua:       ua,
		breakers: newBreakerSet(),
		quota:    newQuota(quotaPerHour, time.Hour),
	}
}

// Handler is the HTTP API; also usable for tests over TCP.
func (d *Daemon) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("POST /resolve", d.handleResolve)
	mux.HandleFunc("POST /refresh", d.handleRefresh)
	mux.HandleFunc("GET /state", d.handleState)
	return mux
}

// hostOf extracts the upstream domain of a target URL.
func hostOf(target string) string {
	u, err := url.Parse(target)
	if err != nil {
		return ""
	}
	return u.Host
}

// Serve listens on the unix socket and blocks.
func (d *Daemon) Serve(sockPath string) error {
	if err := os.Remove(sockPath); err != nil && !os.IsNotExist(err) {
		return err
	}
	ln, err := net.Listen("unix", sockPath)
	if err != nil {
		return err
	}
	return http.Serve(ln, d.Handler())
}

type targetRequest struct {
	Target string `json:"target"`
}

type linkResponse struct {
	Direct    string `json:"direct"`
	ExpiresAt int64  `json:"expires_at"`
	Cached    bool   `json:"cached"`
}

func (d *Daemon) handleResolve(w http.ResponseWriter, r *http.Request) {
	var req targetRequest
	if !decode(w, r, &req) {
		return
	}
	if entry, err := d.cache.Get(req.Target); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	} else if entry != nil {
		writeJSON(w, linkResponse{Direct: entry.Direct, ExpiresAt: entry.ExpiresAt, Cached: true})
		return
	}

	// gatekeeping for scan-driven resolutions only
	now := time.Now()
	domain := hostOf(req.Target)
	if allowed, _ := d.breakers.get(domain).Allow(now, false); !allowed {
		http.Error(w, "breaker open for "+domain, http.StatusServiceUnavailable)
		return
	}
	if skip, err := d.cache.ShouldSkip(req.Target, now); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	} else if skip {
		http.Error(w, "target backing off", http.StatusTooManyRequests)
		return
	}
	if !d.quota.Allow(domain, now) {
		http.Error(w, "quota exceeded for "+domain, http.StatusTooManyRequests)
		return
	}
	d.resolveAndStore(r.Context(), w, req.Target, false)
}

// refresh re-resolves unconditionally; called when playback hit a dead
// link. Never called in bulk.
func (d *Daemon) handleRefresh(w http.ResponseWriter, r *http.Request) {
	var req targetRequest
	if !decode(w, r, &req) {
		return
	}
	d.resolveAndStore(r.Context(), w, req.Target, true)
}

func (d *Daemon) handleState(w http.ResponseWriter, _ *http.Request) {
	stats, err := d.cache.Stats()
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	failures, _ := d.cache.FailureCount()
	writeJSON(w, map[string]any{
		"entries":  stats.Entries,
		"hits":     stats.Hits,
		"failures": failures,
		"breakers": d.breakers.snapshot(),
		"quota":    d.quota.usage(time.Now()),
	})
}

func (d *Daemon) resolveAndStore(ctx context.Context, w http.ResponseWriter, target string, force bool) {
	now := time.Now()
	domain := hostOf(target)
	link, err := linkres.Resolve(ctx, target, d.ua)
	if err != nil {
		d.breakers.get(domain).RecordFailure(now)
		if err := d.cache.RecordFailure(target, now); err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		http.Error(w, err.Error(), http.StatusBadGateway)
		return
	}
	d.breakers.get(domain).RecordSuccess()
	if err := d.cache.ClearFailure(target); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	if err := d.cache.Put(&Entry{
		Target:    target,
		Direct:    link.URL,
		UA:        d.ua,
		ExpiresAt: link.ExpiresAt,
	}); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	writeJSON(w, linkResponse{Direct: link.URL, ExpiresAt: link.ExpiresAt, Cached: force})
}

func decode(w http.ResponseWriter, r *http.Request, v any) bool {
	if err := json.NewDecoder(r.Body).Decode(v); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return false
	}
	return true
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(v)
}
