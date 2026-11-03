# Packaging

Files the image adds on top of `lscr.io/linuxserver/plex`. The tree under
`s6-overlay/` mirrors `/etc/s6-overlay/` in the container, so the
Dockerfile copies it with one line:

```dockerfile
COPY packaging/s6-overlay/ /etc/s6-overlay/
```

## Services

linuxserver images boot through [s6-overlay] v3, whose service manager
s6-rc reads the service graph from the file system. Most files here are
empty on purpose: their **name** is the declaration.

| Path under `s6-rc.d/`                              | Meaning                                      |
| -------------------------------------------------- | -------------------------------------------- |
| `assistant-install/type`                           | `oneshot`: runs once at boot                 |
| `assistant-install/up`                             | the command that oneshot runs                |
| `assistant-install/install.sh`                     | installs the scanner and transcoder in place |
| `assistant-install/dependencies.d/init-config-end` | waits until linuxserver has set up `/config` |
| `assistant-runtime/type`                           | `longrun`: supervised, restarted if it exits |
| `assistant-runtime/run`                            | starts the link-cache daemon as user `abc`   |
| `assistant-runtime/dependencies.d/*`               | starts after the install and base services   |
| `svc-plex/dependencies.d/assistant-install`        | Plex itself waits for the install            |
| `user/contents.d/*`                                | adds both services to the default bundle     |

`svc-plex` and `user` belong to the base image. The directories here
only add one dependency to each of them.

Boot order:

```text
init-config-end ──▶ assistant-install ──▶ svc-plex (Plex)
                          │
init-services ────────────┴──────────▶ assistant-runtime (daemon)
```

The install runs before Plex on every boot. It is idempotent, so a
recreated container puts its symlinks back by itself.

## Changing a service

- Scripts must stay executable. Git keeps the mode, so use
  `git update-index --chmod=+x <file>` on systems that lose it.
- The daemon's socket path in `assistant-runtime/run` must equal
  `runtime::DEFAULT_SOCK`, which the scanner and transcoder dial.
- A new service needs a `type`, its script, the dependencies it waits
  for, and an entry in `user/contents.d/` so it starts at all.

`cargo test --test packaging` checks these rules: every service has a
type and an executable script, every dependency names a real service,
and the paths match the Dockerfile and the code. It cannot boot the
container. For that, build the image and check `docker logs` for the
`runtime: listening on` line.

[s6-overlay]: https://github.com/just-containers/s6-overlay
