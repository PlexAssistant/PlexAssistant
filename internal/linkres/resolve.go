package linkres

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"
)

// DefaultUA identifies us to cloud drives; links can be bound to it, so
// scan and playback must use the same value.
const DefaultUA = "PlexAssistant/0.1"

const maxRedirects = 10

// Resolution failure kinds; the scanner maps them to its failure states.
var (
	ErrDead      = errors.New("dead link")
	ErrForbidden = errors.New("forbidden")
	ErrGone      = errors.New("gone")
)

// Link is a resolved direct link plus what the probe learned.
type Link struct {
	URL            string   // final direct link
	Chain          []string // redirect hops taken
	ContentType    string
	Size           int64 // total bytes if known
	RangeSupported bool  // server answered 206 to our range probe
	ExpiresAt      int64 // unix seconds sniffed from the URL, 0 = unknown
}

// Resolve follows the strm URL's redirect chain and returns the direct
// link without downloading the media: it probes with a 1-byte range
// request and reads only headers.
func Resolve(ctx context.Context, rawURL, ua string) (*Link, error) {
	if ua == "" {
		ua = DefaultUA
	}

	link := &Link{}
	client := &http.Client{
		CheckRedirect: func(req *http.Request, via []*http.Request) error {
			link.Chain = append(link.Chain, req.URL.String())
			if len(via) >= maxRedirects {
				return fmt.Errorf("too many redirects")
			}
			return nil
		},
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, rawURL, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", ua)
	req.Header.Set("Range", "bytes=0-0")

	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer func() { _ = resp.Body.Close() }()

	switch resp.StatusCode {
	case http.StatusOK, http.StatusPartialContent:
	case http.StatusNotFound:
		return nil, ErrDead
	case http.StatusForbidden:
		return nil, ErrForbidden
	case http.StatusGone:
		return nil, ErrGone
	default:
		return nil, fmt.Errorf("unexpected status %d", resp.StatusCode)
	}

	link.URL = resp.Request.URL.String()
	link.ContentType = resp.Header.Get("Content-Type")
	link.RangeSupported = resp.StatusCode == http.StatusPartialContent
	link.ExpiresAt = SniffExpiry(link.URL, time.Now())
	if total := contentRangeTotal(resp.Header.Get("Content-Range")); total > 0 {
		link.Size = total
	} else {
		link.Size, _ = strconv.ParseInt(resp.Header.Get("Content-Length"), 10, 64)
	}
	return link, nil
}

// SniffExpiry looks for a plausible unix-seconds timestamp in the URL
// query (signed links commonly carry exp/expires/t=...). It is
// informational only; returned 0 means unknown.
func SniffExpiry(rawURL string, now time.Time) int64 {
	u, err := url.Parse(rawURL)
	if err != nil {
		return 0
	}
	var best int64
	for _, values := range u.Query() {
		for _, v := range values {
			if len(v) != 10 {
				continue // signatures are longer or hex
			}
			n, err := strconv.ParseInt(v, 10, 64)
			if err != nil || n <= now.Unix() {
				continue
			}
			if best == 0 || n > best {
				best = n
			}
		}
	}
	return best
}

// contentRangeTotal parses "bytes 0-0/12345" into 12345.
func contentRangeTotal(v string) int64 {
	_, total, ok := strings.Cut(v, "/")
	if !ok {
		return 0
	}
	n, err := strconv.ParseInt(total, 10, 64)
	if err != nil {
		return 0
	}
	return n
}
