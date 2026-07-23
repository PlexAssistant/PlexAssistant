default:
    @just --list

build:
    go build -o assistant ./cmd/assistant

build-linux:
    GOOS=linux GOARCH=arm64 CGO_ENABLED=0 go build -o dist/assistant-linux-arm64 ./cmd/assistant

test:
    go test ./...

vet:
    go vet ./...

lint:
    golangci-lint run

fmt:
    goimports -w .
    go mod tidy

image:
    docker build -t plexassistant:dev .

gen-media base="http://fakecloud:8399" out="testdata":
    ./scripts/gen-media.sh {{base}} {{out}}

gen-bulk count="500" base="http://fakecloud:8399" out="testdata":
    ./scripts/gen-bulk-strm.sh {{count}} {{base}} {{out}}

rig-up: build-linux
    docker compose -f compose.dev.yaml up -d

rig-down:
    docker compose -f compose.dev.yaml down

# rebuild the binary, ship it in, re-install and bounce the daemon
deploy: build-linux
    docker compose -f compose.dev.yaml cp dist/assistant-linux-arm64 plex:/usr/local/bin/assistant
    docker compose -f compose.dev.yaml exec -T plex chmod +x /usr/local/bin/assistant
    docker compose -f compose.dev.yaml exec -T plex /usr/local/bin/assistant install
    docker compose -f compose.dev.yaml exec -T plex s6-svc -r /run/service/assistant-runtime

undeploy:
    docker compose -f compose.dev.yaml exec -T plex /usr/local/bin/assistant install revert
