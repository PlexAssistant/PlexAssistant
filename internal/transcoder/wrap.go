// Package transcoder wraps the Plex Transcoder: strm inputs are swapped
// for cached direct links before handing off to the original binary, so
// playback neither re-resolves the gateway URL nor bypasses the cache.
package transcoder

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/PlexAssistant/PlexAssistant/internal/linkres"
	"github.com/PlexAssistant/PlexAssistant/internal/runtime"
)

// Resolver supplies direct links; implemented by the daemon client with
// a direct fallback.
type Resolver interface {
	Resolve(ctx context.Context, target string) (string, error)
	Refresh(ctx context.Context, target string) (string, error)
}

// DaemonResolver asks the runtime daemon and falls back to resolving
// directly when it is unreachable.
type DaemonResolver struct {
	SockPath  string
	UserAgent string
}

// Resolve returns a direct link for the target URL.
func (d DaemonResolver) Resolve(ctx context.Context, target string) (string, error) {
	if d.SockPath != "" {
		client := &runtime.Client{SockPath: d.SockPath}
		if res, err := client.Resolve(ctx, target); err == nil {
			return res.Direct, nil
		}
	}
	link, err := linkres.Resolve(ctx, target, d.UserAgent)
	if err != nil {
		return "", err
	}
	return link.URL, nil
}

// Refresh forces a fresh link (playback hit a dead one).
func (d DaemonResolver) Refresh(ctx context.Context, target string) (string, error) {
	if d.SockPath != "" {
		client := &runtime.Client{SockPath: d.SockPath}
		if res, err := client.Refresh(ctx, target); err == nil {
			return res.Direct, nil
		}
	}
	link, err := linkres.Resolve(ctx, target, d.UserAgent)
	if err != nil {
		return "", err
	}
	return link.URL, nil
}

// Swap records one argv input replacement.
type Swap struct {
	Target   string // strm URL
	Direct   string // link placed into argv
	ArgIndex int    // argv position of the replaced input
}

// RewriteArgs replaces every "-i <path>.strm" input with a resolved
// direct link.
func RewriteArgs(ctx context.Context, args []string, res Resolver) ([]string, []Swap) {
	out := append([]string(nil), args...)
	var swaps []Swap
	for i := 0; i+1 < len(out); i++ {
		if out[i] != "-i" || !strings.HasSuffix(strings.ToLower(out[i+1]), ".strm") {
			continue
		}
		target, err := linkres.ParseSTRM(out[i+1])
		if err != nil || !target.IsRemote {
			continue
		}
		direct, err := res.Resolve(ctx, target.Value)
		if err != nil || direct == "" {
			continue // leave the strm; the original handles it
		}
		out[i+1] = direct
		swaps = append(swaps, Swap{Target: target.Value, Direct: direct, ArgIndex: i + 1})
	}
	return out, swaps
}

// fastFailWindow bounds how quickly the original must fail before we
// suspect a dead link and retry once with refreshed ones.
const fastFailWindow = 30 * time.Second

// Run rewrites the args, runs the preserved original as a child with
// signal relaying, and retries once with refreshed links when it dies
// fast with our swap in place. It returns the final exit code.
func Run(args []string, realPath string, res Resolver) int {
	ctx := context.Background()
	rewritten, swaps := RewriteArgs(ctx, args, res)

	start := time.Now()
	code := runChild(realPath, rewritten)
	if code == 0 || len(swaps) == 0 || time.Since(start) > fastFailWindow {
		return code
	}

	// Fast failure with our link in argv: probably dead. Refresh every
	// swapped target and retry once — the playback-time self-heal path.
	refreshed := false
	for _, s := range swaps {
		fresh, err := res.Refresh(ctx, s.Target)
		if err != nil || fresh == "" || fresh == s.Direct {
			continue
		}
		rewritten[s.ArgIndex] = fresh
		refreshed = true
	}
	if !refreshed {
		return code
	}
	return runChild(realPath, rewritten)
}

// runChild starts the original, forwards every signal, waits and
// returns its exit code.
func runChild(realPath string, args []string) int {
	cmd := exec.Command(realPath, args...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	if err := cmd.Start(); err != nil {
		fmt.Fprintf(os.Stderr, "transcoder wrap: %v\n", err)
		return 1
	}

	sigs := make(chan os.Signal, 8)
	signal.Notify(sigs)
	done := make(chan struct{})
	go func() {
		for {
			select {
			case s := <-sigs:
				if cmd.Process != nil {
					_ = cmd.Process.Signal(s)
				}
			case <-done:
				return
			}
		}
	}()
	err := cmd.Wait()
	close(done)
	signal.Stop(sigs)

	if err == nil {
		return 0
	}
	var ee *exec.ExitError
	if errors.As(err, &ee) {
		if status, ok := ee.Sys().(syscall.WaitStatus); ok && status.Signaled() {
			return 128 + int(status.Signal())
		}
		return ee.ExitCode()
	}
	return 1
}
