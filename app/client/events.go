package client

import (
	"context"
	"log"

	pb "github.com/waifu-devs/fuwa/client/proto"
)

type EventHandler struct {
	manager     *Manager
	subscribers map[string]context.CancelFunc
	MessageChan chan *pb.Message
	ChannelChan chan *pb.Channel
}

func NewEventHandler(manager *Manager) *EventHandler {
	return &EventHandler{
		manager:     manager,
		subscribers: make(map[string]context.CancelFunc),
		MessageChan: make(chan *pb.Message, 100),
		ChannelChan: make(chan *pb.Channel, 100),
	}
}

func (e *EventHandler) Subscribe(serverID string) error {
	clients, exists := e.manager.GetClients(serverID)
	if !exists {
		return ErrServerNotConnected
	}

	ctx, cancel := context.WithCancel(context.Background())
	e.subscribers[serverID] = cancel

	go func() {
		defer func() {
			if r := recover(); r != nil {
				log.Printf("Event subscription for server %s recovered from panic: %v", serverID, r)
			}
		}()

		stream, err := clients.Event.Subscribe(ctx, &pb.SubscribeRequest{
			EventTypes: []string{"message.sent", "channel.created", "channel.updated"},
			Scopes:     []string{"server:" + serverID},
		})
		if err != nil {
			log.Printf("Failed to subscribe to events for server %s: %v", serverID, err)
			return
		}

		for {
			select {
			case <-ctx.Done():
				return
			default:
				event, err := stream.Recv()
				if err != nil {
					log.Printf("Event stream error for server %s: %v", serverID, err)
					return
				}

				e.handleEvent(event)
			}
		}
	}()

	return nil
}

func (e *EventHandler) handleEvent(event *pb.Event) {
	switch event.EventType {
	case "message.sent":
		// For simplicity, we'll create a mock message from the event
		// In a real implementation, you'd properly deserialize the payload
		message := &pb.Message{
			MessageId: event.EventId,
			Content:   "Event: " + event.EventType,
			AuthorId:  event.ActorId,
		}

		select {
		case e.MessageChan <- message:
		default:
			log.Println("Message channel full, dropping message event")
		}

	case "channel.created", "channel.updated":
		// Mock channel event
		channel := &pb.Channel{
			ChannelId: event.EventId,
			Name:      "Event Channel",
			ServerId:  extractServerIdFromScope(event.Scope),
		}

		select {
		case e.ChannelChan <- channel:
		default:
			log.Println("Channel channel full, dropping channel event")
		}
	}
}

func (e *EventHandler) Unsubscribe(serverID string) {
	if cancel, exists := e.subscribers[serverID]; exists {
		cancel()
		delete(e.subscribers, serverID)
		log.Printf("Unsubscribed from events for server %s", serverID)
	}
}

func (e *EventHandler) Close() {
	for serverID, cancel := range e.subscribers {
		cancel()
		log.Printf("Closed event subscription for server %s", serverID)
	}
	e.subscribers = make(map[string]context.CancelFunc)
	close(e.MessageChan)
	close(e.ChannelChan)
}

func extractServerIdFromScope(scope string) string {
	if len(scope) > 7 && scope[:7] == "server:" {
		return scope[7:]
	}
	return scope
}
