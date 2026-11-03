#!/usr/bin/with-contenv bash
# Oneshot, before Plex starts: point Plex's scanner and transcoder at the
# assistant and set the scan preferences. Install is idempotent, so it
# runs on every boot and a recreated container heals itself.
exec /usr/local/bin/assistant install
