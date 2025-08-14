# Fuwa Protocol Buffers

This directory contains the gRPC/protobuf definitions for the Fuwa service, designed around events as the core primitive for horizontal scalability.

## Design Philosophy

### Event-Driven Architecture
- **Events as Primitives**: Everything in Fuwa is represented as an event
- **Horizontal Scalability**: Events can be distributed across multiple nodes
- **Simple Design**: Minimal complexity while maintaining flexibility
- **Pub/Sub Model**: Services subscribe to relevant events and publish new ones

### Core Concepts

#### Event Structure
```protobuf
message Event {
  string event_id = 1;        // Unique identifier
  string event_type = 2;      // "channel.created", "message.sent", etc.
  string scope = 3;           // Namespace for filtering ("server:123")
  string actor_id = 4;        // Who triggered the event
  Timestamp timestamp = 5;    // When it occurred
  Any payload = 6;           // Event-specific data
  map<string, string> metadata = 7; // Additional context
  int64 sequence = 8;        // Ordering within scope
}
```

#### Scoping and Filtering
Events use hierarchical scoping for efficient filtering:
- `server:123` - All events for server 123
- `channel:456` - All events for channel 456
- `user:789` - All events for user 789

#### Sequence Numbers
Each scope maintains its own sequence counter for:
- Event ordering within scope
- Catching up on missed events
- Ensuring consistency

## Services

### EventService (Core)
The foundation service that all others build upon:
- `Subscribe()` - Stream events with filtering
- `Publish()` - Emit events to the system
- `GetEvents()` - Retrieve historical events

### Domain Services
Built on top of the event system:
- `ChannelService` - Channel CRUD operations
- `MessageService` - Message operations

## Usage Examples

### Subscribing to Channel Events
```go
req := &SubscribeRequest{
    EventTypes: []string{"channel.created", "channel.updated"},
    Scopes: []string{"server:123"},
}
stream, err := eventClient.Subscribe(ctx, req)
```

### Publishing a Message Event
```go
payload := &MessageSentPayload{Message: message}
event := &Event{
    EventType: "message.sent",
    Scope: "channel:456",
    ActorId: "user:789",
    Payload: mustMarshalAny(payload),
}
_, err := eventClient.Publish(ctx, &PublishRequest{Event: event})
```

## Scalability Features

1. **Event Sharding**: Events can be partitioned by scope
2. **Service Decoupling**: Services only need to know about relevant events
3. **Load Distribution**: Multiple subscribers can process events in parallel
4. **State Reconstruction**: Services can rebuild state from event history
5. **Eventual Consistency**: Services update asynchronously via events

## Generation

Generate Go code from protobuf files:
```bash
make proto-gen
```

Clean generated files:
```bash
make proto-clean
```

## Requirements

- `protoc` (Protocol Buffers compiler)
- `protoc-gen-go` (Go protocol buffers plugin)
- `protoc-gen-go-grpc` (Go gRPC plugin)

Install with:
```bash
go install google.golang.org/protobuf/cmd/protoc-gen-go@latest
go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@latest
```
