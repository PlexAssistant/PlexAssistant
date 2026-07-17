// Command fakecloud is a fake cloud drive for the test rig: /file/{id}
// resolves to a signed direct link via 302, /dl/{id} serves the bytes with
// injectable failure modes (403/410/throttle) controlled at runtime via
// POST /ctl.
package main

import (
	"flag"
	"log"
	"net/http"
	"time"
)

func main() {
	addr := flag.String("addr", ":8399", "listen address")
	root := flag.String("root", "testdata/media", "media root")
	secret := flag.String("secret", "testrig", "token signing secret")
	ttl := flag.Duration("ttl", time.Hour, "default direct-link TTL")
	flag.Parse()

	log.Printf("fakecloud listening on %s, root %s", *addr, *root)
	log.Fatal(http.ListenAndServe(*addr, newServer(*secret, *root, *ttl).mux()))
}
