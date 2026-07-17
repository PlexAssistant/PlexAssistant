package main

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	behaviorNormal    = "normal"
	behaviorForbidden = "forbidden" // /dl answers 403
	behaviorGone      = "gone"      // /dl answers 410
	behaviorSlow      = "slow"      // /dl answers throttled
)

type server struct {
	secret []byte
	root   string

	mu       sync.Mutex
	behavior map[string]string        // id -> behavior
	bps      map[string]int64         // id -> bytes per second for slow
	ttl      map[string]time.Duration // id -> link TTL
	defTTL   time.Duration
}

func newServer(secret, root string, defTTL time.Duration) *server {
	return &server{
		secret:   []byte(secret),
		root:     root,
		behavior: map[string]string{},
		bps:      map[string]int64{},
		ttl:      map[string]time.Duration{},
		defTTL:   defTTL,
	}
}

func (s *server) mux() *http.ServeMux {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /file/{id}", s.handleFile)
	mux.HandleFunc("GET /dl/{id}", s.handleDL)
	mux.HandleFunc("POST /ctl", s.handleCtl)
	mux.HandleFunc("GET /state", s.handleState)
	return mux
}

// handleFile resolves an id to a fresh signed direct link.
func (s *server) handleFile(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if !validID(id) || !s.exists(id) {
		http.NotFound(w, r)
		return
	}
	exp := time.Now().Add(s.ttlFor(id))
	http.Redirect(w, r, "/dl/"+id+"?t="+s.sign(id, exp), http.StatusFound)
}

// handleDL serves the bytes after token validation and behavior injection.
func (s *server) handleDL(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if !validID(id) {
		http.NotFound(w, r)
		return
	}
	f, err := os.Open(filepath.Join(s.root, id))
	if err != nil {
		http.NotFound(w, r)
		return
	}
	defer func() { _ = f.Close() }()

	exp, ok := s.verify(id, r.URL.Query().Get("t"))
	if !ok {
		http.Error(w, "invalid token", http.StatusForbidden)
		return
	}
	if time.Now().After(exp) {
		http.Error(w, "link expired", http.StatusGone)
		return
	}
	switch s.behaviorFor(id) {
	case behaviorForbidden:
		http.Error(w, "forbidden", http.StatusForbidden)
		return
	case behaviorGone:
		http.Error(w, "gone", http.StatusGone)
		return
	}
	var rs io.ReadSeeker = f
	if bps := s.bpsFor(id); bps > 0 {
		rs = &throttleReader{r: f, bps: bps}
	}
	http.ServeContent(w, r, id, time.Now(), rs)
}

type ctlRequest struct {
	ID       string `json:"id"`       // file id, or "*" for global reset/defaults
	Behavior string `json:"behavior"` // normal | forbidden | gone | slow
	BPS      int64  `json:"bps"`      // bytes per second for slow
	TTL      string `json:"ttl"`      // link TTL, e.g. "10s"
}

func (s *server) handleCtl(w http.ResponseWriter, r *http.Request) {
	var req ctlRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	var d time.Duration
	if req.TTL != "" {
		var err error
		if d, err = time.ParseDuration(req.TTL); err != nil || d < 0 {
			http.Error(w, "bad ttl", http.StatusBadRequest)
			return
		}
	}
	if req.Behavior != "" && !validBehavior(req.Behavior) {
		http.Error(w, "bad behavior", http.StatusBadRequest)
		return
	}

	s.mu.Lock()
	defer s.mu.Unlock()
	if req.ID == "*" {
		if req.Behavior == behaviorNormal {
			s.behavior = map[string]string{}
			s.bps = map[string]int64{}
		}
		if req.TTL != "" {
			s.defTTL = d
		}
	} else {
		if req.Behavior != "" {
			s.behavior[req.ID] = req.Behavior
		}
		if req.BPS > 0 {
			s.bps[req.ID] = req.BPS
		} else {
			delete(s.bps, req.ID)
		}
		if req.TTL != "" {
			s.ttl[req.ID] = d
		}
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *server) handleState(w http.ResponseWriter, _ *http.Request) {
	s.mu.Lock()
	defer s.mu.Unlock()
	_ = json.NewEncoder(w).Encode(map[string]any{
		"default_ttl": s.defTTL.String(),
		"behavior":    s.behavior,
		"bps":         s.bps,
		"ttl":         s.ttl,
	})
}

func validID(id string) bool {
	return id != "" && id != "." && id != ".." && !strings.ContainsAny(id, `/\`)
}

func validBehavior(b string) bool {
	switch b {
	case behaviorNormal, behaviorForbidden, behaviorGone, behaviorSlow:
		return true
	}
	return false
}

func (s *server) exists(id string) bool {
	_, err := os.Stat(filepath.Join(s.root, id))
	return err == nil
}

func (s *server) ttlFor(id string) time.Duration {
	s.mu.Lock()
	defer s.mu.Unlock()
	if d, ok := s.ttl[id]; ok {
		return d
	}
	return s.defTTL
}

func (s *server) behaviorFor(id string) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.behavior[id]
}

func (s *server) bpsFor(id string) int64 {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.bps[id]
}

// sign returns "<unix-exp>.<hmac8>" over id and exp.
func (s *server) sign(id string, exp time.Time) string {
	mac := hmac.New(sha256.New, s.secret)
	_, _ = mac.Write([]byte(id + "|" + strconv.FormatInt(exp.Unix(), 10)))
	return strconv.FormatInt(exp.Unix(), 10) + "." + hex.EncodeToString(mac.Sum(nil)[:8])
}

// verify checks the token signature and returns the expiry.
func (s *server) verify(id, token string) (time.Time, bool) {
	expStr, _, ok := strings.Cut(token, ".")
	if !ok {
		return time.Time{}, false
	}
	exp, err := strconv.ParseInt(expStr, 10, 64)
	if err != nil {
		return time.Time{}, false
	}
	if !hmac.Equal([]byte(s.sign(id, time.Unix(exp, 0))), []byte(token)) {
		return time.Time{}, false
	}
	return time.Unix(exp, 0), true
}

// throttleReader caps read speed at bps, sending a chunk every 100ms.
type throttleReader struct {
	r   io.ReadSeeker
	bps int64
}

func (t *throttleReader) Read(p []byte) (int, error) {
	chunk := t.bps / 10
	if chunk < 1 {
		chunk = 1
	}
	if int64(len(p)) > chunk {
		p = p[:chunk]
	}
	n, err := t.r.Read(p)
	if n > 0 {
		time.Sleep(100 * time.Millisecond)
	}
	return n, err
}

func (t *throttleReader) Seek(offset int64, whence int) (int64, error) {
	return t.r.Seek(offset, whence)
}
