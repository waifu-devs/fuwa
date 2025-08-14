# Fuwa gRPC Server & Client Generation

This guide shows you how to generate and run both the gRPC server and client for Fuwa.

## Quick Start

### 1. Install Dependencies

First, install the required protobuf generation tools:

```bash
make install-deps
```

This will install:
- `protoc-gen-go` - Go protobuf code generator
- `protoc-gen-go-grpc` - Go gRPC code generator

### 2. Generate Server & Client Code

Generate Go code from the protobuf definitions:

```bash
make proto-gen
```

This creates:
- `server/proto/` - Server-side generated code
- `client/proto/` - Client-side generated code

### 3. Run the Server

Start the gRPC server:

```bash
make run-server
```

The server will start on `localhost:50051` and implement:
- **EventService** - Core event pub/sub system
- **ChannelService** - Channel management
- **MessageService** - Message handling

### 4. Run the Client (in another terminal)

Start the example client:

```bash
make run-client
```

The client will connect to the server and demonstrate:
- Creating a channel
- Sending a message
- Subscribing to events
- Publishing events

## Manual Steps

If you prefer to run things manually:

### Generate Protobuf Code
```bash
# For server
mkdir -p server/proto
protoc --go_out=server/proto --go_opt=paths=source_relative \
       --go-grpc_out=server/proto --go-grpc_opt=paths=source_relative \
       --proto_path=proto proto/*.proto

# For client
mkdir -p client/proto
protoc --go_out=client/proto --go_opt=paths=source_relative \
       --go-grpc_out=client/proto --go-grpc_opt=paths=source_relative \
       --proto_path=proto proto/*.proto
```

### Run Server
```bash
cd server && go run ./cmd/grpc-server
```

### Run Client
```bash
cd client && go run ./cmd/example-client
```

## Build Binaries

Create standalone binaries:

```bash
# Build server binary
make build-server
# Creates: bin/fuwa-server

# Build client binary
make build-client
# Creates: bin/fuwa-client

# Run binaries directly
./bin/fuwa-server
./bin/fuwa-client
```

## Generated Code Structure

After running `make proto-gen`, you'll have:

```
server/proto/
├── fuwa.pb.go         # Message definitions
└── fuwa_grpc.pb.go    # Service interfaces (server-side)

client/proto/
├── fuwa.pb.go         # Message definitions
└── fuwa_grpc.pb.go    # Service interfaces (client-side)
```

## Using the Generated Code

### Server Implementation

The generated server code provides interfaces you implement:

```go
import pb "github.com/waifu-devs/fuwa/server/proto"

// Implement the EventService
type eventServiceServer struct {
    pb.UnimplementedEventServiceServer
}

func (s *eventServiceServer) Subscribe(req *pb.SubscribeRequest, stream pb.EventService_SubscribeServer) error {
    // Your event subscription logic here
    return nil
}

// Register with gRPC server
pb.RegisterEventServiceServer(grpcServer, &eventServiceServer{})
```

### Client Usage

The generated client code provides client stubs:

```go
import pb "github.com/waifu-devs/fuwa/client/proto"

// Connect to server
conn, err := grpc.NewClient("localhost:50051", grpc.WithTransportCredentials(insecure.NewCredentials()))
eventClient := pb.NewEventServiceClient(conn)

// Make calls
resp, err := eventClient.Publish(ctx, &pb.PublishRequest{
    Event: &pb.Event{
        EventType: "message.sent",
        Scope: "channel:123",
    },
})
```

## Debugging

### Test Server Connectivity

Use grpcurl to test the server:

```bash
# Install grpcurl
go install github.com/fullstorydev/grpcurl/cmd/grpcurl@latest

# List services (server must be running)
grpcurl -plaintext localhost:50051 list

# Call a method
grpcurl -plaintext -d '{"name":"test"}' localhost:50051 fuwa.ChannelService/CreateChannel
```

### View Generated Code

Examine the generated protobuf interfaces:

```bash
# View server interfaces
ls -la server/proto/
cat server/proto/fuwa_grpc.pb.go

# View client interfaces
ls -la client/proto/
cat client/proto/fuwa_grpc.pb.go
```

## Clean Up

Remove generated files:

```bash
make proto-clean
```

## Makefile Targets

| Target | Description |
|--------|-------------|
| `install-deps` | Install protobuf generation tools |
| `proto-gen` | Generate Go code for server & client |
| `proto-clean` | Remove generated protobuf files |
| `build-server` | Build server binary |
| `build-client` | Build client binary |
| `run-server` | Run server (generates proto first) |
| `run-client` | Run example client (generates proto first) |
| `help` | Show available targets |
