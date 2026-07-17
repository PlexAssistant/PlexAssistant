package install

import (
	"encoding/xml"
	"fmt"
	"os"
	"path/filepath"
)

// installPrefs are set at install time so library scans react to file
// changes immediately.
var installPrefs = map[string]string{
	"FSEventLibraryUpdatesEnabled":     "1",
	"FSEventLibraryPartialScanEnabled": "1",
}

// DefaultSupportDir is the Plex data directory inside the container.
const DefaultSupportDir = "/config/Library/Application Support/Plex Media Server"

// setPrefs ensures our preference attributes are present on the
// Preferences.xml root element, preserving every other attribute.
// A missing file is not an error (Plex has not booted yet).
func setPrefs(supportDir string) error {
	path := filepath.Join(supportDir, "Preferences.xml")
	raw, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil {
		return err
	}

	var root struct {
		XMLName xml.Name
		Attrs   []xml.Attr `xml:",any,attr"`
	}
	if err := xml.Unmarshal(raw, &root); err != nil {
		return fmt.Errorf("parse %s: %w", path, err)
	}

	have := map[string]string{}
	for _, a := range root.Attrs {
		have[a.Name.Local] = a.Value
	}
	changed := false
	for k, v := range installPrefs {
		if have[k] != v {
			changed = true
		}
	}
	if !changed {
		return nil
	}

	for k, v := range installPrefs {
		found := false
		for i, a := range root.Attrs {
			if a.Name.Local == k {
				root.Attrs[i].Value = v
				found = true
				break
			}
		}
		if !found {
			root.Attrs = append(root.Attrs, xml.Attr{
				Name:  xml.Name{Local: k},
				Value: v,
			})
		}
	}

	out, err := xml.Marshal(&root)
	if err != nil {
		return err
	}
	tmp := path + ".assistant-tmp"
	if err := os.WriteFile(tmp, append(out, '\n'), 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}
