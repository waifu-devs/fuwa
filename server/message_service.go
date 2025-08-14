package server

import (
	"context"
	"database/sql"
	"fmt"
	"log"
	"strconv"
	"time"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
	"google.golang.org/protobuf/types/known/timestamppb"

	"github.com/waifu-devs/fuwa/server/database"
	pb "github.com/waifu-devs/fuwa/server/proto"
)

type messageServiceServer struct {
	pb.UnimplementedMessageServiceServer
	db           *database.Queries
	eventService *eventServiceServer
}

func NewMessageServiceServer(db *database.Queries, eventService *eventServiceServer) *messageServiceServer {
	return &messageServiceServer{
		db:           db,
		eventService: eventService,
	}
}

func (s *messageServiceServer) SendMessage(ctx context.Context, req *pb.SendMessageRequest) (*pb.SendMessageResponse, error) {
	if req.ChannelId == "" {
		return nil, status.Error(codes.InvalidArgument, "channel_id is required")
	}
	if req.Content == "" && len(req.Attachments) == 0 && len(req.Embeds) == 0 {
		return nil, status.Error(codes.InvalidArgument, "message must have content, attachments, or embeds")
	}

	// Generate message ID
	messageID := fmt.Sprintf("message_%d", time.Now().UnixNano())
	now := time.Now().Unix()

	// Create message in database
	dbMessage, err := s.db.CreateMessage(ctx, database.CreateMessageParams{
		MessageID: messageID,
		ChannelID: req.ChannelId,
		AuthorID:  getActorFromContext(ctx), // TODO: Get from auth context
		Content:   req.Content,
		CreatedAt: now,
		UpdatedAt: now,
		ReplyToID: sql.NullString{String: req.ReplyToId, Valid: req.ReplyToId != ""},
	})
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to create message: %v", err)
	}

	// Handle attachments
	var attachments []*pb.Attachment
	for _, attachment := range req.Attachments {
		attachmentID := fmt.Sprintf("attachment_%d", time.Now().UnixNano())
		_, err := s.db.CreateAttachment(ctx, database.CreateAttachmentParams{
			AttachmentID: attachmentID,
			MessageID:    messageID,
			Filename:     attachment.Filename,
			ContentType:  attachment.ContentType,
			Size:         attachment.Size,
			Url:          attachment.Url,
		})
		if err != nil {
			log.Printf("Failed to save attachment: %v", err)
			continue
		}

		// Add ID to response attachment
		attachment.AttachmentId = attachmentID
		attachments = append(attachments, attachment)
	}

	// Handle embeds
	var embeds []*pb.Embed
	for _, embed := range req.Embeds {
		embedID := time.Now().UnixNano()
		_, err := s.db.CreateEmbed(ctx, database.CreateEmbedParams{
			EmbedID:      embedID,
			MessageID:    messageID,
			Title:        sql.NullString{String: embed.Title, Valid: embed.Title != ""},
			Description:  sql.NullString{String: embed.Description, Valid: embed.Description != ""},
			Url:          sql.NullString{String: embed.Url, Valid: embed.Url != ""},
			Color:        sql.NullInt64{Int64: int64(embed.Color), Valid: embed.Color != 0},
			ThumbnailUrl: sql.NullString{String: embed.ThumbnailUrl, Valid: embed.ThumbnailUrl != ""},
			ImageUrl:     sql.NullString{String: embed.ImageUrl, Valid: embed.ImageUrl != ""},
		})
		if err != nil {
			log.Printf("Failed to save embed: %v", err)
			continue
		}

		// Handle embed fields
		for _, field := range embed.Fields {
			fieldID := time.Now().UnixNano()
			_, err := s.db.CreateEmbedField(ctx, database.CreateEmbedFieldParams{
				FieldID: fieldID,
				EmbedID: embedID,
				Name:    field.Name,
				Value:   field.Value,
				Inline:  boolToInt64(field.Inline),
			})
			if err != nil {
				log.Printf("Failed to save embed field: %v", err)
			}
		}

		embeds = append(embeds, embed)
	}

	// Convert to proto message
	protoMessage := dbMessageToProto(&dbMessage)
	protoMessage.Attachments = attachments
	protoMessage.Embeds = embeds

	// Publish message.sent event
	if s.eventService != nil {
		eventID := fmt.Sprintf("message-sent-%d", time.Now().UnixNano())
		event := &pb.Event{
			EventId:   eventID,
			EventType: "message.sent",
			Scope:     fmt.Sprintf("channel:%s", req.ChannelId),
			ActorId:   getActorFromContext(ctx),
			Timestamp: timestamppb.Now(),
			Metadata: map[string]string{
				"message_id": messageID,
				"channel_id": req.ChannelId,
			},
			Sequence: time.Now().Unix(),
		}

		_, err = s.eventService.Publish(ctx, &pb.PublishRequest{Event: event})
		if err != nil {
			log.Printf("Failed to publish message.sent event: %v", err)
		}
	}

	return &pb.SendMessageResponse{
		Message: protoMessage,
	}, nil
}

func (s *messageServiceServer) GetMessage(ctx context.Context, req *pb.GetMessageRequest) (*pb.GetMessageResponse, error) {
	if req.MessageId == "" {
		return nil, status.Error(codes.InvalidArgument, "message_id is required")
	}

	dbMessage, err := s.db.GetMessage(ctx, req.MessageId)
	if err != nil {
		if err == sql.ErrNoRows {
			return nil, status.Error(codes.NotFound, "message not found")
		}
		return nil, status.Errorf(codes.Internal, "failed to get message: %v", err)
	}

	// Get attachments
	attachments, err := s.getMessageAttachments(ctx, req.MessageId)
	if err != nil {
		log.Printf("Failed to get attachments: %v", err)
	}

	// Get embeds
	embeds, err := s.getMessageEmbeds(ctx, req.MessageId)
	if err != nil {
		log.Printf("Failed to get embeds: %v", err)
	}

	protoMessage := dbMessageToProto(&dbMessage)
	protoMessage.Attachments = attachments
	protoMessage.Embeds = embeds

	return &pb.GetMessageResponse{
		Message: protoMessage,
	}, nil
}

func (s *messageServiceServer) GetMessages(ctx context.Context, req *pb.GetMessagesRequest) (*pb.GetMessagesResponse, error) {
	if req.ChannelId == "" {
		return nil, status.Error(codes.InvalidArgument, "channel_id is required")
	}

	limit := int64(50) // Default limit
	if req.Limit > 0 && req.Limit <= 100 {
		limit = int64(req.Limit)
	}

	// Handle pagination with before_id and after_id
	var beforeTime, afterTime int64
	if req.BeforeId != "" {
		// Parse timestamp from ID (simplified - in production you'd want better ID design)
		if id, err := strconv.ParseInt(req.BeforeId[8:], 10, 64); err == nil {
			beforeTime = id
		}
	}
	if req.AfterId != "" {
		if id, err := strconv.ParseInt(req.AfterId[8:], 10, 64); err == nil {
			afterTime = id
		}
	}

	dbMessages, err := s.db.GetMessages(ctx, database.GetMessagesParams{
		ChannelID:   req.ChannelId,
		CreatedAt:   beforeTime,
		Column3:     beforeTime,
		CreatedAt_2: afterTime,
		Column5:     afterTime,
		Limit:       limit,
	})
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to get messages: %v", err)
	}

	messages := make([]*pb.Message, len(dbMessages))
	for i, dbMessage := range dbMessages {
		protoMessage := dbMessageToProto(&dbMessage)

		// Get attachments and embeds for each message
		attachments, _ := s.getMessageAttachments(ctx, dbMessage.MessageID)
		embeds, _ := s.getMessageEmbeds(ctx, dbMessage.MessageID)

		protoMessage.Attachments = attachments
		protoMessage.Embeds = embeds
		messages[i] = protoMessage
	}

	// Check if there are more messages
	hasMore := len(messages) == int(limit)

	return &pb.GetMessagesResponse{
		Messages: messages,
		HasMore:  hasMore,
	}, nil
}

func (s *messageServiceServer) UpdateMessage(ctx context.Context, req *pb.UpdateMessageRequest) (*pb.UpdateMessageResponse, error) {
	if req.MessageId == "" {
		return nil, status.Error(codes.InvalidArgument, "message_id is required")
	}

	// Get existing message first
	existingMessage, err := s.db.GetMessage(ctx, req.MessageId)
	if err != nil {
		if err == sql.ErrNoRows {
			return nil, status.Error(codes.NotFound, "message not found")
		}
		return nil, status.Errorf(codes.Internal, "failed to get message: %v", err)
	}

	// Update message
	dbMessage, err := s.db.UpdateMessage(ctx, database.UpdateMessageParams{
		Content:   req.Content,
		UpdatedAt: time.Now().Unix(),
		MessageID: req.MessageId,
	})
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to update message: %v", err)
	}

	// Handle embed updates (simplified - delete and recreate)
	if len(req.Embeds) > 0 {
		// TODO: Delete existing embeds and create new ones
		// This is a simplified implementation
	}

	protoMessage := dbMessageToProto(&dbMessage)
	protoMessage.Embeds = req.Embeds

	// Publish message.updated event
	if s.eventService != nil {
		eventID := fmt.Sprintf("message-updated-%d", time.Now().UnixNano())
		event := &pb.Event{
			EventId:   eventID,
			EventType: "message.updated",
			Scope:     fmt.Sprintf("channel:%s", existingMessage.ChannelID),
			ActorId:   getActorFromContext(ctx),
			Timestamp: timestamppb.Now(),
			Metadata: map[string]string{
				"message_id": req.MessageId,
				"channel_id": existingMessage.ChannelID,
			},
			Sequence: time.Now().Unix(),
		}

		_, err = s.eventService.Publish(ctx, &pb.PublishRequest{Event: event})
		if err != nil {
			log.Printf("Failed to publish message.updated event: %v", err)
		}
	}

	return &pb.UpdateMessageResponse{
		Message: protoMessage,
	}, nil
}

func (s *messageServiceServer) DeleteMessage(ctx context.Context, req *pb.DeleteMessageRequest) (*pb.DeleteMessageResponse, error) {
	if req.MessageId == "" {
		return nil, status.Error(codes.InvalidArgument, "message_id is required")
	}

	// Get message info before deletion for event
	existingMessage, err := s.db.GetMessage(ctx, req.MessageId)
	if err != nil {
		if err == sql.ErrNoRows {
			return nil, status.Error(codes.NotFound, "message not found")
		}
		return nil, status.Errorf(codes.Internal, "failed to get message: %v", err)
	}

	// Delete message (this should cascade to attachments and embeds)
	err = s.db.DeleteMessage(ctx, req.MessageId)
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to delete message: %v", err)
	}

	// Publish message.deleted event
	if s.eventService != nil {
		eventID := fmt.Sprintf("message-deleted-%d", time.Now().UnixNano())
		event := &pb.Event{
			EventId:   eventID,
			EventType: "message.deleted",
			Scope:     fmt.Sprintf("channel:%s", existingMessage.ChannelID),
			ActorId:   getActorFromContext(ctx),
			Timestamp: timestamppb.Now(),
			Metadata: map[string]string{
				"message_id": req.MessageId,
				"channel_id": existingMessage.ChannelID,
			},
			Sequence: time.Now().Unix(),
		}

		_, err = s.eventService.Publish(ctx, &pb.PublishRequest{Event: event})
		if err != nil {
			log.Printf("Failed to publish message.deleted event: %v", err)
		}
	}

	return &pb.DeleteMessageResponse{
		Success: true,
	}, nil
}

func (s *messageServiceServer) getMessageAttachments(ctx context.Context, messageID string) ([]*pb.Attachment, error) {
	dbAttachments, err := s.db.GetAttachmentsByMessageId(ctx, messageID)
	if err != nil {
		return nil, err
	}

	attachments := make([]*pb.Attachment, len(dbAttachments))
	for i, dbAttachment := range dbAttachments {
		attachments[i] = &pb.Attachment{
			AttachmentId: dbAttachment.AttachmentID,
			Filename:     dbAttachment.Filename,
			ContentType:  dbAttachment.ContentType,
			Size:         dbAttachment.Size,
			Url:          dbAttachment.Url,
		}
	}

	return attachments, nil
}

func (s *messageServiceServer) getMessageEmbeds(ctx context.Context, messageID string) ([]*pb.Embed, error) {
	dbEmbeds, err := s.db.GetEmbedsByMessageId(ctx, messageID)
	if err != nil {
		return nil, err
	}

	embeds := make([]*pb.Embed, len(dbEmbeds))
	for i, dbEmbed := range dbEmbeds {
		// Get embed fields
		dbFields, _ := s.db.GetEmbedFieldsByEmbedId(ctx, dbEmbed.EmbedID)

		fields := make([]*pb.EmbedField, len(dbFields))
		for j, dbField := range dbFields {
			fields[j] = &pb.EmbedField{
				Name:   dbField.Name,
				Value:  dbField.Value,
				Inline: int64ToBool(dbField.Inline),
			}
		}

		embeds[i] = &pb.Embed{
			Title:        dbEmbed.Title.String,
			Description:  dbEmbed.Description.String,
			Url:          dbEmbed.Url.String,
			Color:        int32(dbEmbed.Color.Int64),
			Fields:       fields,
			ThumbnailUrl: dbEmbed.ThumbnailUrl.String,
			ImageUrl:     dbEmbed.ImageUrl.String,
		}
	}

	return embeds, nil
}

// Helper function to convert database message to proto message
func dbMessageToProto(dbMessage *database.Message) *pb.Message {
	return &pb.Message{
		MessageId: dbMessage.MessageID,
		ChannelId: dbMessage.ChannelID,
		AuthorId:  dbMessage.AuthorID,
		Content:   dbMessage.Content,
		CreatedAt: timestamppb.New(time.Unix(dbMessage.CreatedAt, 0)),
		UpdatedAt: timestamppb.New(time.Unix(dbMessage.UpdatedAt, 0)),
		ReplyToId: dbMessage.ReplyToID.String,
	}
}

func boolToInt64(b bool) int64 {
	if b {
		return 1
	}
	return 0
}

func int64ToBool(i int64) bool {
	return i != 0
}
