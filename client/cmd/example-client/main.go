package main

import (
	"context"
	"log"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"

	pb "github.com/waifu-devs/fuwa/client/proto"
)

func main() {
	// Connect to the gRPC server
	conn, err := grpc.NewClient("localhost:50051", grpc.WithTransportCredentials(insecure.NewCredentials()))
	if err != nil {
		log.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	// Create service clients
	eventClient := pb.NewEventServiceClient(conn)
	channelClient := pb.NewChannelServiceClient(conn)
	messageClient := pb.NewMessageServiceClient(conn)

	ctx, cancel := context.WithTimeout(context.Background(), time.Second*10)
	defer cancel()

	// Example: Create a channel
	log.Println("=== Creating a channel ===")
	createChannelResp, err := channelClient.CreateChannel(ctx, &pb.CreateChannelRequest{
		Name:     "general",
		Type:     pb.ChannelType_CHANNEL_TYPE_TEXT,
		ServerId: "server-123",
	})
	if err != nil {
		log.Fatalf("CreateChannel failed: %v", err)
	}
	log.Printf("Created channel: %+v", createChannelResp.Channel)

	// Example: Send a message
	log.Println("\n=== Sending a message ===")
	sendMessageResp, err := messageClient.SendMessage(ctx, &pb.SendMessageRequest{
		ChannelId: createChannelResp.Channel.ChannelId,
		Content:   "Hello from the gRPC client!",
	})
	if err != nil {
		log.Fatalf("SendMessage failed: %v", err)
	}
	log.Printf("Sent message: %+v", sendMessageResp.Message)

	// Example: Subscribe to events (streaming)
	log.Println("\n=== Subscribing to events ===")
	stream, err := eventClient.Subscribe(ctx, &pb.SubscribeRequest{
		EventTypes: []string{"channel.created", "message.sent"},
		Scopes:     []string{"server:123"},
	})
	if err != nil {
		log.Fatalf("Subscribe failed: %v", err)
	}

	// Listen for events (this will receive the test event from the server)
	for {
		event, err := stream.Recv()
		if err != nil {
			log.Printf("Stream ended: %v", err)
			break
		}
		log.Printf("Received event: %+v", event)

		// Exit after receiving one event for demo purposes
		break
	}

	// Example: Publish an event
	log.Println("\n=== Publishing an event ===")
	publishResp, err := eventClient.Publish(ctx, &pb.PublishRequest{
		Event: &pb.Event{
			EventId:   "client-event-1",
			EventType: "user.action",
			Scope:     "server:123",
			ActorId:   "user:456",
			Sequence:  1,
		},
	})
	if err != nil {
		log.Fatalf("Publish failed: %v", err)
	}
	log.Printf("Published event response: %+v", publishResp)

	log.Println("\n=== Client demo completed ===")
}
