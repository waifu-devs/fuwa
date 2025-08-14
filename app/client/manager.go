package client

import (
	"context"
	"errors"
	"log"
	"sync"

	pb "github.com/waifu-devs/fuwa/client/proto"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

var (
	ErrServerNotConnected = errors.New("server not connected")
	ErrNoServerAvailable  = errors.New("no server available")
)

type Manager struct {
	connections map[string]*grpc.ClientConn
	clients     map[string]*Clients
	mu          sync.RWMutex
}

type Clients struct {
	Event   pb.EventServiceClient
	Channel pb.ChannelServiceClient
	Message pb.MessageServiceClient
	Config  pb.ConfigServiceClient
}

func NewManager() *Manager {
	return &Manager{
		connections: make(map[string]*grpc.ClientConn),
		clients:     make(map[string]*Clients),
	}
}

func (m *Manager) Connect(serverID, address string) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if _, exists := m.connections[serverID]; exists {
		return nil
	}

	conn, err := grpc.NewClient(address, grpc.WithTransportCredentials(insecure.NewCredentials()))
	if err != nil {
		return err
	}

	clients := &Clients{
		Event:   pb.NewEventServiceClient(conn),
		Channel: pb.NewChannelServiceClient(conn),
		Message: pb.NewMessageServiceClient(conn),
		Config:  pb.NewConfigServiceClient(conn),
	}

	m.connections[serverID] = conn
	m.clients[serverID] = clients

	log.Printf("Connected to server %s at %s", serverID, address)
	return nil
}

func (m *Manager) Disconnect(serverID string) {
	m.mu.Lock()
	defer m.mu.Unlock()

	if conn, exists := m.connections[serverID]; exists {
		conn.Close()
		delete(m.connections, serverID)
		delete(m.clients, serverID)
		log.Printf("Disconnected from server %s", serverID)
	}
}

func (m *Manager) GetClients(serverID string) (*Clients, bool) {
	m.mu.RLock()
	defer m.mu.RUnlock()
	clients, exists := m.clients[serverID]
	return clients, exists
}

func (m *Manager) ListChannels(ctx context.Context, serverID string) ([]*pb.Channel, error) {
	clients, exists := m.GetClients(serverID)
	if !exists {
		return nil, ErrServerNotConnected
	}

	resp, err := clients.Channel.ListChannels(ctx, &pb.ListChannelsRequest{
		ServerId: serverID,
	})
	if err != nil {
		return nil, err
	}

	return resp.Channels, nil
}

func (m *Manager) GetMessages(ctx context.Context, channelID string, limit int32) ([]*pb.Message, error) {
	for _, clients := range m.clients {
		resp, err := clients.Message.GetMessages(ctx, &pb.GetMessagesRequest{
			ChannelId: channelID,
			Limit:     limit,
		})
		if err == nil {
			return resp.Messages, nil
		}
	}
	return nil, ErrNoServerAvailable
}

func (m *Manager) SendMessage(ctx context.Context, channelID, content string) (*pb.Message, error) {
	for _, clients := range m.clients {
		resp, err := clients.Message.SendMessage(ctx, &pb.SendMessageRequest{
			ChannelId: channelID,
			Content:   content,
		})
		if err == nil {
			return resp.Message, nil
		}
	}
	return nil, ErrNoServerAvailable
}

func (m *Manager) CreateChannel(ctx context.Context, serverID, channelName string, channelType pb.ChannelType) (*pb.Channel, error) {
	clients, exists := m.GetClients(serverID)
	if !exists {
		return nil, ErrServerNotConnected
	}

	resp, err := clients.Channel.CreateChannel(ctx, &pb.CreateChannelRequest{
		Name:     channelName,
		Type:     channelType,
		ServerId: serverID,
	})
	if err != nil {
		return nil, err
	}

	return resp.Channel, nil
}

func (m *Manager) Close() {
	m.mu.Lock()
	defer m.mu.Unlock()

	for serverID, conn := range m.connections {
		conn.Close()
		log.Printf("Closed connection to server %s", serverID)
	}

	m.connections = make(map[string]*grpc.ClientConn)
	m.clients = make(map[string]*Clients)
}
