.PHONY: proto-gen proto-clean help install-deps build-server build-client build-app run-server run-client run-app

# Install required protobuf dependencies
install-deps:
	@echo "Installing protobuf dependencies..."
	@go install google.golang.org/protobuf/cmd/protoc-gen-go@latest
	@go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@latest
	@echo "✓ Dependencies installed"

# Generate Go code from protobuf files (both server and client)
proto-gen:
	@echo "Generating Go code from protobuf files..."
	@mkdir -p server/proto
	@mkdir -p client/proto
	@protoc --go_out=server/proto --go_opt=paths=source_relative \
		--go-grpc_out=server/proto --go-grpc_opt=paths=source_relative \
		--proto_path=proto proto/*.proto
	@protoc --go_out=client/proto --go_opt=paths=source_relative \
		--go-grpc_out=client/proto --go-grpc_opt=paths=source_relative \
		--proto_path=proto proto/*.proto
	@echo "✓ Server and client protobuf generation completed"

# Clean generated protobuf files
proto-clean:
	@echo "Cleaning generated protobuf files..."
	@rm -rf server/proto
	@rm -rf client/proto
	@echo "✓ Cleanup completed"

# Build server binary
build-server: proto-gen
	@echo "Building server..."
	@cd server && go build -o ../bin/fuwa-server ./cmd
	@echo "✓ Server built as bin/fuwa-server"

# Build client binary
build-client: proto-gen
	@echo "Building client..."
	@cd client && go build -o ../bin/fuwa-client ./cmd/example-client
	@echo "✓ Client built as bin/fuwa-client"

# Build app binary
build-app: proto-gen
	@echo "Building Discord-like app..."
	@cd app && go build -o ../bin/fuwa-app .
	@echo "✓ App built as bin/fuwa-app"

# Run server (generates protobuf code first)
run-server: proto-gen
	@echo "Starting Fuwa gRPC server..."
	@cd server && go run ./cmd

# Run client (generates protobuf code first)
run-client: proto-gen
	@echo "Starting Fuwa example client..."
	@cd client && go run ./cmd/example-client

# Run app (generates protobuf code first)
run-app: proto-gen
	@echo "Starting Fuwa Discord-like app..."
	@cd app && go run .

# Display help
help:
	@echo "Available targets:"
	@echo "  install-deps - Install required protobuf generation tools"
	@echo "  proto-gen    - Generate Go code from .proto files (server & client)"
	@echo "  proto-clean  - Clean generated protobuf files"
	@echo "  build-server - Build server binary to bin/fuwa-server"
	@echo "  build-client - Build client binary to bin/fuwa-client"
	@echo "  build-app    - Build Discord-like app to bin/fuwa-app"
	@echo "  run-server   - Run the gRPC server"
	@echo "  run-client   - Run the example client"
	@echo "  run-app      - Run the Discord-like app"
	@echo "  help         - Show this help message"
