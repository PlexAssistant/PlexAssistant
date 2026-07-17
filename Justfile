default:
    @just --list

build:
    go build -o assistant ./cmd/assistant

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

rig-up:
    docker compose -f docker-compose.dev.yml up -d

rig-down:
    docker compose -f docker-compose.dev.yml down

# cross-build for the rig container and install into Plex
deploy:
    GOOS=linux GOARCH=arm64 CGO_ENABLED=0 go build -o dist/assistant-linux-arm64 ./cmd/assistant
    docker compose -f docker-compose.dev.yml cp dist/assistant-linux-arm64 plex:/usr/local/bin/assistant
    docker compose -f docker-compose.dev.yml exec -T plex chmod +x /usr/local/bin/assistant
    docker compose -f docker-compose.dev.yml exec -T plex /usr/local/bin/assistant install

undeploy:
    docker compose -f docker-compose.dev.yml exec -T plex /usr/local/bin/assistant install revert
