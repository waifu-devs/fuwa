package server

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"log"
	"time"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
	"google.golang.org/protobuf/types/known/timestamppb"

	"github.com/waifu-devs/fuwa/server/database"
	pb "github.com/waifu-devs/fuwa/server/proto"
)

type channelServiceServer struct {
	pb.UnimplementedChannelServiceServer
	db           *database.Queries
	eventService *eventServiceServer
}

func NewChannelServiceServer(db *database.Queries, eventService *eventServiceServer) *channelServiceServer {
	return &channelServiceServer{
		db:           db,
		eventService: eventService,
	}
}

func (s *channelServiceServer) CreateChannel(ctx context.Context, req *pb.CreateChannelRequest) (*pb.CreateChannelResponse, error) {
	if req.Name == "" {
		return nil, status.Error(codes.InvalidArgument, "channel name is required")
	}
	if req.ServerId == "" {
		return nil, status.Error(codes.InvalidArgument, "server_id is required")
	}

	// Generate channel ID
	channelID := fmt.Sprintf("channel_%d", time.Now().UnixNano())
	now := time.Now().Unix()

	// Convert metadata to JSON
	var metadataJSON string
	if req.Metadata != nil && len(req.Metadata) > 0 {
		metadataBytes, err := json.Marshal(req.Metadata)
		if err != nil {
			return nil, status.Errorf(codes.Internal, "failed to marshal metadata: %v", err)
		}
		metadataJSON = string(metadataBytes)
	}

	// Create channel in database
	dbChannel, err := s.db.CreateChannel(ctx, database.CreateChannelParams{
		ChannelID: channelID,
		Name:      req.Name,
		Type:      int64(req.Type),
		ServerID:  sql.NullString{String: req.ServerId, Valid: req.ServerId != ""},
		ParentID:  sql.NullString{String: req.ParentId, Valid: req.ParentId != ""},
		Metadata:  sql.NullString{String: metadataJSON, Valid: metadataJSON != ""},
		CreatedAt: now,
		UpdatedAt: now,
	})
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to create channel: %v", err)
	}

	// Convert to proto message
	protoChannel := dbChannelToProto(&dbChannel)

	// Publish channel.created event
	if s.eventService != nil {
		eventID := fmt.Sprintf("channel-created-%d", time.Now().UnixNano())
		event := &pb.Event{
			EventId:   eventID,
			EventType: "channel.created",
			Scope:     fmt.Sprintf("server:%s", req.ServerId),
			ActorId:   getActorFromContext(ctx),
			Timestamp: timestamppb.Now(),
			Metadata: map[string]string{
				"channel_id":   channelID,
				"channel_name": req.Name,
			},
			Sequence: time.Now().Unix(),
		}

		_, err = s.eventService.Publish(ctx, &pb.PublishRequest{Event: event})
		if err != nil {
			log.Printf("Failed to publish channel.created event: %v", err)
		}
	}

	return &pb.CreateChannelResponse{
		Channel: protoChannel,
	}, nil
}

func (s *channelServiceServer) GetChannel(ctx context.Context, req *pb.GetChannelRequest) (*pb.GetChannelResponse, error) {
	if req.ChannelId == "" {
		return nil, status.Error(codes.InvalidArgument, "channel_id is required")
	}

	dbChannel, err := s.db.GetChannel(ctx, req.ChannelId)
	if err != nil {
		if err == sql.ErrNoRows {
			return nil, status.Error(codes.NotFound, "channel not found")
		}
		return nil, status.Errorf(codes.Internal, "failed to get channel: %v", err)
	}

	return &pb.GetChannelResponse{
		Channel: dbChannelToProto(&dbChannel),
	}, nil
}

func (s *channelServiceServer) ListChannels(ctx context.Context, req *pb.ListChannelsRequest) (*pb.ListChannelsResponse, error) {
	limit := int64(50) // Default limit
	if req.Limit > 0 && req.Limit <= 100 {
		limit = int64(req.Limit)
	}

	offset := int64(0)
	if req.PageToken != "" {
		// Simple offset-based pagination (in production, you might want cursor-based)
		// For now, assume page_token is the offset as string
		fmt.Sscanf(req.PageToken, "%d", &offset)
	}

	dbChannels, err := s.db.ListChannels(ctx, database.ListChannelsParams{
		ServerID: sql.NullString{String: req.ServerId, Valid: req.ServerId != ""},
		Column2:  req.ServerId,
		ParentID: sql.NullString{String: req.ParentId, Valid: req.ParentId != ""},
		Column4:  req.ParentId,
		Limit:    limit,
		Offset:   offset,
	})
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to list channels: %v", err)
	}

	channels := make([]*pb.Channel, len(dbChannels))
	for i, dbChannel := range dbChannels {
		channels[i] = dbChannelToProto(&dbChannel)
	}

	// Calculate next page token
	var nextPageToken string
	if len(channels) == int(limit) {
		nextPageToken = fmt.Sprintf("%d", offset+limit)
	}

	return &pb.ListChannelsResponse{
		Channels:      channels,
		NextPageToken: nextPageToken,
	}, nil
}

func (s *channelServiceServer) UpdateChannel(ctx context.Context, req *pb.UpdateChannelRequest) (*pb.UpdateChannelResponse, error) {
	if req.ChannelId == "" {
		return nil, status.Error(codes.InvalidArgument, "channel_id is required")
	}

	// Get existing channel first
	existingChannel, err := s.db.GetChannel(ctx, req.ChannelId)
	if err != nil {
		if err == sql.ErrNoRows {
			return nil, status.Error(codes.NotFound, "channel not found")
		}
		return nil, status.Errorf(codes.Internal, "failed to get channel: %v", err)
	}

	// Prepare update parameters
	name := existingChannel.Name
	metadata := existingChannel.Metadata.String

	// Apply updates based on update_mask
	for _, field := range req.UpdateMask {
		switch field {
		case "name":
			name = req.Name
		case "metadata":
			if req.Metadata != nil {
				metadataBytes, err := json.Marshal(req.Metadata)
				if err != nil {
					return nil, status.Errorf(codes.Internal, "failed to marshal metadata: %v", err)
				}
				metadata = string(metadataBytes)
			}
		}
	}

	// Update channel
	dbChannel, err := s.db.UpdateChannel(ctx, database.UpdateChannelParams{
		Name:      name,
		Metadata:  sql.NullString{String: metadata, Valid: metadata != ""},
		UpdatedAt: time.Now().Unix(),
		ChannelID: req.ChannelId,
	})
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to update channel: %v", err)
	}

	// Publish channel.updated event
	if s.eventService != nil {
		eventID := fmt.Sprintf("channel-updated-%d", time.Now().UnixNano())
		event := &pb.Event{
			EventId:   eventID,
			EventType: "channel.updated",
			Scope:     fmt.Sprintf("server:%s", existingChannel.ServerID.String),
			ActorId:   getActorFromContext(ctx),
			Timestamp: timestamppb.Now(),
			Metadata: map[string]string{
				"channel_id":     req.ChannelId,
				"changed_fields": fmt.Sprintf("%v", req.UpdateMask),
			},
			Sequence: time.Now().Unix(),
		}

		_, err = s.eventService.Publish(ctx, &pb.PublishRequest{Event: event})
		if err != nil {
			log.Printf("Failed to publish channel.updated event: %v", err)
		}
	}

	return &pb.UpdateChannelResponse{
		Channel: dbChannelToProto(&dbChannel),
	}, nil
}

func (s *channelServiceServer) DeleteChannel(ctx context.Context, req *pb.DeleteChannelRequest) (*pb.DeleteChannelResponse, error) {
	if req.ChannelId == "" {
		return nil, status.Error(codes.InvalidArgument, "channel_id is required")
	}

	// Get channel info before deletion for event
	existingChannel, err := s.db.GetChannel(ctx, req.ChannelId)
	if err != nil {
		if err == sql.ErrNoRows {
			return nil, status.Error(codes.NotFound, "channel not found")
		}
		return nil, status.Errorf(codes.Internal, "failed to get channel: %v", err)
	}

	// Delete channel
	err = s.db.DeleteChannel(ctx, req.ChannelId)
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to delete channel: %v", err)
	}

	// Publish channel.deleted event
	if s.eventService != nil {
		eventID := fmt.Sprintf("channel-deleted-%d", time.Now().UnixNano())
		event := &pb.Event{
			EventId:   eventID,
			EventType: "channel.deleted",
			Scope:     fmt.Sprintf("server:%s", existingChannel.ServerID.String),
			ActorId:   getActorFromContext(ctx),
			Timestamp: timestamppb.Now(),
			Metadata: map[string]string{
				"channel_id":   req.ChannelId,
				"channel_name": existingChannel.Name,
			},
			Sequence: time.Now().Unix(),
		}

		_, err = s.eventService.Publish(ctx, &pb.PublishRequest{Event: event})
		if err != nil {
			log.Printf("Failed to publish channel.deleted event: %v", err)
		}
	}

	return &pb.DeleteChannelResponse{
		Success: true,
	}, nil
}

// Helper function to convert database channel to proto channel
func dbChannelToProto(dbChannel *database.Channel) *pb.Channel {
	var metadata map[string]string
	if dbChannel.Metadata.Valid && dbChannel.Metadata.String != "" {
		json.Unmarshal([]byte(dbChannel.Metadata.String), &metadata)
	}

	return &pb.Channel{
		ChannelId: dbChannel.ChannelID,
		Name:      dbChannel.Name,
		Type:      pb.ChannelType(dbChannel.Type),
		ServerId:  dbChannel.ServerID.String,
		ParentId:  dbChannel.ParentID.String,
		Metadata:  metadata,
		CreatedAt: timestamppb.New(time.Unix(dbChannel.CreatedAt, 0)),
		UpdatedAt: timestamppb.New(time.Unix(dbChannel.UpdatedAt, 0)),
	}
}

func getActorFromContext(ctx context.Context) string {
	// TODO: Extract user ID from JWT token or similar
	return "system"
}
