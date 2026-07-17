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

rig-up:
    docker compose up -d

rig-down:
    docker compose down
