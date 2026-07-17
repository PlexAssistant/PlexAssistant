package runtime

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"time"
)

// Client talks to the daemon over its unix socket. Callers fall back to
// resolving directly when the daemon is not running.
type Client struct {
	SockPath string
}

// Resolved is the daemon's answer.
type Resolved struct {
	Direct    string `json:"direct"`
	ExpiresAt int64  `json:"expires_at"`
	Cached    bool   `json:"cached"`
}

func (c *Client) post(ctx context.Context, method, target string) (*Resolved, error) {
	body, _ := json.Marshal(targetRequest{Target: target})
	req, err := http.NewRequestWithContext(ctx, http.MethodPost,
		"http://assistant/"+method, bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	client := &http.Client{
		Timeout: 30 * time.Second,
		Transport: &http.Transport{
			DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
				var d net.Dialer
				return d.DialContext(ctx, "unix", c.SockPath)
			},
		},
	}
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("daemon %s: status %d", method, resp.StatusCode)
	}
	var out Resolved
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		return nil, err
	}
	return &out, nil
}

// Resolve returns the cached or freshly resolved link.
func (c *Client) Resolve(ctx context.Context, target string) (*Resolved, error) {
	return c.post(ctx, "resolve", target)
}

// Refresh forces a re-resolve (dead link during playback).
func (c *Client) Refresh(ctx context.Context, target string) (*Resolved, error) {
	return c.post(ctx, "refresh", target)
}
