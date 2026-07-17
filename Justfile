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
