package main

import (
	"log"
	"net"

	"google.golang.org/grpc"
	"google.golang.org/grpc/reflection"

	"github.com/waifu-devs/fuwa/server"
	"github.com/waifu-devs/fuwa/server/database"
	pb "github.com/waifu-devs/fuwa/server/proto"
)

func main() {
	// Load configuration
	config, err := server.LoadConfig()
	if err != nil {
		log.Fatalf("Failed to load config: %v", err)
	}

	log.Printf("Starting Fuwa server with config: %v", config)

	// Set up multi-database manager
	dbManager := server.NewMultiDatabaseManager(config)
	defer dbManager.Close()

	// Read all database files in the data path
	if err := dbManager.ReadAllDatabases(); err != nil {
		log.Fatalf("Failed to initialize databases: %v", err)
	}

	// Get primary database queries instance (will auto-create if none exist)
	var queries *database.Queries
	queries, err = dbManager.GetPrimaryQueries()
	if err != nil {
		log.Fatalf("Failed to get database queries: %v", err)
	}

	if queries == nil {
		log.Printf("Warning: Running without database connections")
	}

	// Create services
	eventService := server.NewEventServiceServer(queries)
	channelService := server.NewChannelServiceServer(queries, eventService)
	messageService := server.NewMessageServiceServer(queries, eventService)
	configService := server.NewConfigServiceServer(config, eventService, nil) // TODO: Implement ConfigStore

	// Set up gRPC server
	lis, err := net.Listen("tcp", ":50051")
	if err != nil {
		log.Fatalf("Failed to listen: %v", err)
	}

	s := grpc.NewServer()

	// Register all services
	pb.RegisterEventServiceServer(s, eventService)
	pb.RegisterChannelServiceServer(s, channelService)
	pb.RegisterMessageServiceServer(s, messageService)
	pb.RegisterConfigServiceServer(s, configService)

	// Enable reflection for tools like grpcurl
	reflection.Register(s)

	log.Println("Fuwa gRPC server starting on :50051")
	log.Println("Services registered: EventService, ChannelService, MessageService, ConfigService")
	if len(dbManager.ListDatabases()) > 0 {
		log.Printf("Connected databases: %v", dbManager.ListDatabases())
	} else {
		log.Printf("No databases connected")
	}

	if err := s.Serve(lis); err != nil {
		log.Fatalf("Failed to serve: %v", err)
	}
}
