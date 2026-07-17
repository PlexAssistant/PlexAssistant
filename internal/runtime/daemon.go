package runtime

import (
	"context"
	"encoding/json"
	"net"
	"net/http"
	"os"

	"github.com/PlexAssistant/PlexAssistant/internal/linkres"
)

// Daemon serves the link cache over a unix socket. Semantics: a link is
// resolved once and then trusted forever; refresh happens only when a
// caller reports it dead while playing.
type Daemon struct {
	cache *Cache
	ua    string
}

// NewDaemon wires a daemon over cache, resolving with ua.
func NewDaemon(cache *Cache, ua string) *Daemon {
	return &Daemon{cache: cache, ua: ua}
}

// Handler is the HTTP API; also usable for tests over TCP.
func (d *Daemon) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("POST /resolve", d.handleResolve)
	mux.HandleFunc("POST /refresh", d.handleRefresh)
	mux.HandleFunc("GET /state", d.handleState)
	return mux
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
	writeJSON(w, stats)
}

func (d *Daemon) resolveAndStore(ctx context.Context, w http.ResponseWriter, target string, refresh bool) {
	link, err := linkres.Resolve(ctx, target, d.ua)
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadGateway)
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
	writeJSON(w, linkResponse{Direct: link.URL, ExpiresAt: link.ExpiresAt, Cached: refresh})
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
