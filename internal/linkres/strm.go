package linkres

import (
	"bytes"
	"fmt"
	"os"
	"strings"
)

// ParseSTRM extracts the target from a .strm file: the first
// non-empty, non-comment line. Local paths are returned with
// IsRemote=false so the caller can leave them to Plex.
func ParseSTRM(path string) (Target, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return Target{}, err
	}
	b = bytes.TrimPrefix(b, []byte{0xEF, 0xBB, 0xBF}) // strip BOM
	for _, line := range strings.Split(string(b), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		return Target{Value: line, IsRemote: isRemote(line)}, nil
	}
	return Target{}, fmt.Errorf("%s: no target line", path)
}

// Target is the parsed content of a .strm file.
type Target struct {
	Value    string
	IsRemote bool // http(s) URL vs local path
}

func isRemote(v string) bool {
	return strings.HasPrefix(v, "http://") || strings.HasPrefix(v, "https://")
}
