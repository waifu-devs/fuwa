package server

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"log"
	"sync"
	"time"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
	"google.golang.org/protobuf/types/known/timestamppb"

	"github.com/waifu-devs/fuwa/server/database"
	pb "github.com/waifu-devs/fuwa/server/proto"
)

type eventServiceServer struct {
	pb.UnimplementedEventServiceServer
	db          *database.Queries
	subscribers map[string]*eventSubscriber
	mu          sync.RWMutex
}

type eventSubscriber struct {
	eventTypes []string
	scopes     []string
	filters    map[string]string
	stream     pb.EventService_SubscribeServer
	done       chan struct{}
}

func NewEventServiceServer(db *database.Queries) *eventServiceServer {
	return &eventServiceServer{
		db:          db,
		subscribers: make(map[string]*eventSubscriber),
	}
}

func (s *eventServiceServer) Subscribe(req *pb.SubscribeRequest, stream pb.EventService_SubscribeServer) error {
	subscriberID := fmt.Sprintf("subscriber_%d", time.Now().UnixNano())

	subscriber := &eventSubscriber{
		eventTypes: req.EventTypes,
		scopes:     req.Scopes,
		filters:    req.Filters,
		stream:     stream,
		done:       make(chan struct{}),
	}

	s.mu.Lock()
	s.subscribers[subscriberID] = subscriber
	s.mu.Unlock()

	// Clean up on disconnect
	defer func() {
		s.mu.Lock()
		delete(s.subscribers, subscriberID)
		s.mu.Unlock()
		close(subscriber.done)
	}()

	log.Printf("Client subscribed: %s", subscriberID)

	// If client wants historical events
	if req.FromSequence != 0 {
		err := s.sendHistoricalEvents(stream, req)
		if err != nil {
			return err
		}
	}

	// Keep connection alive and wait for disconnect
	<-stream.Context().Done()
	log.Printf("Client unsubscribed: %s", subscriberID)
	return nil
}

func (s *eventServiceServer) Publish(ctx context.Context, req *pb.PublishRequest) (*pb.PublishResponse, error) {
	if req.Event == nil {
		return nil, status.Error(codes.InvalidArgument, "event is required")
	}

	event := req.Event
	if event.EventId == "" {
		event.EventId = fmt.Sprintf("event_%d", time.Now().UnixNano())
	}
	if event.Timestamp == nil {
		event.Timestamp = timestamppb.Now()
	}

	// Get next sequence number for this scope
	nextSequence, err := s.getNextSequence(ctx, event.Scope)
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to get next sequence: %v", err)
	}
	event.Sequence = nextSequence

	// Convert payload to JSON if present
	var payloadJSON string
	if event.Payload != nil {
		payloadBytes, err := json.Marshal(event.Payload)
		if err != nil {
			return nil, status.Errorf(codes.Internal, "failed to marshal payload: %v", err)
		}
		payloadJSON = string(payloadBytes)
	}

	// Convert metadata to JSON
	var metadataJSON string
	if event.Metadata != nil && len(event.Metadata) > 0 {
		metadataBytes, err := json.Marshal(event.Metadata)
		if err != nil {
			return nil, status.Errorf(codes.Internal, "failed to marshal metadata: %v", err)
		}
		metadataJSON = string(metadataBytes)
	}

	// Store event in database
	_, err = s.db.CreateEvent(ctx, database.CreateEventParams{
		EventID:   event.EventId,
		EventType: event.EventType,
		Scope:     event.Scope,
		ActorID:   event.ActorId,
		Timestamp: event.Timestamp.AsTime().Unix(),
		Payload:   sql.NullString{String: payloadJSON, Valid: payloadJSON != ""},
		Metadata:  sql.NullString{String: metadataJSON, Valid: metadataJSON != ""},
		Sequence:  event.Sequence,
	})
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to store event: %v", err)
	}

	// Broadcast to subscribers
	s.broadcastEvent(event)

	return &pb.PublishResponse{
		EventId:  event.EventId,
		Sequence: event.Sequence,
		Success:  true,
	}, nil
}

func (s *eventServiceServer) GetEvents(ctx context.Context, req *pb.GetEventsRequest) (*pb.GetEventsResponse, error) {
	if req.Scope == "" {
		return nil, status.Error(codes.InvalidArgument, "scope is required")
	}

	limit := int64(50) // Default limit
	if req.Limit > 0 && req.Limit <= 100 {
		limit = int64(req.Limit)
	}

	fromSeq := req.FromSequence
	if fromSeq < 0 {
		fromSeq = 0
	}

	toSeq := req.ToSequence
	if toSeq <= 0 {
		// Get latest sequence if not specified
		latestSeqInterface, err := s.db.GetLatestSequence(ctx, req.Scope)
		if err != nil {
			return nil, status.Errorf(codes.Internal, "failed to get latest sequence: %v", err)
		}

		if latestSeqInterface != nil {
			if seq, ok := latestSeqInterface.(int64); ok {
				toSeq = seq
			} else {
				toSeq = 0
			}
		} else {
			toSeq = 0
		}
	}

	// Get events from database
	dbEvents, err := s.db.GetEvents(ctx, database.GetEventsParams{
		Scope:      req.Scope,
		EventTypes: req.EventTypes,
		Sequence:   fromSeq,
		Sequence_2: toSeq,
		Limit:      limit,
	})
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to get events: %v", err)
	}

	// Convert to proto events
	events := make([]*pb.Event, len(dbEvents))
	for i, dbEvent := range dbEvents {
		events[i] = dbEventToProto(&dbEvent)
	}

	// Check if there are more events
	hasMore := len(events) == int(limit)
	var nextSequence int64
	if hasMore && len(events) > 0 {
		nextSequence = events[len(events)-1].Sequence + 1
	}

	return &pb.GetEventsResponse{
		Events:       events,
		HasMore:      hasMore,
		NextSequence: nextSequence,
	}, nil
}

func (s *eventServiceServer) sendHistoricalEvents(stream pb.EventService_SubscribeServer, req *pb.SubscribeRequest) error {
	// For each scope the client is interested in
	scopes := req.Scopes
	if len(scopes) == 0 {
		// If no scopes specified, we can't send historical events
		return nil
	}

	for _, scope := range scopes {
		// Get events from the requested sequence
		events, err := s.GetEvents(stream.Context(), &pb.GetEventsRequest{
			Scope:        scope,
			EventTypes:   req.EventTypes,
			FromSequence: req.FromSequence,
			Limit:        100, // Reasonable batch size
		})
		if err != nil {
			return err
		}

		// Send each event
		for _, event := range events.Events {
			if s.eventMatchesFilters(event, req) {
				if err := stream.Send(event); err != nil {
					return err
				}
			}
		}
	}

	return nil
}

func (s *eventServiceServer) broadcastEvent(event *pb.Event) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	for subscriberID, subscriber := range s.subscribers {
		if s.eventMatchesSubscriber(event, subscriber) {
			select {
			case <-subscriber.done:
				// Subscriber is done, skip
				continue
			default:
				// Send event
				err := subscriber.stream.Send(event)
				if err != nil {
					log.Printf("Failed to send event to subscriber %s: %v", subscriberID, err)
				}
			}
		}
	}
}

func (s *eventServiceServer) eventMatchesSubscriber(event *pb.Event, subscriber *eventSubscriber) bool {
	// Check event types
	if len(subscriber.eventTypes) > 0 {
		found := false
		for _, eventType := range subscriber.eventTypes {
			if event.EventType == eventType {
				found = true
				break
			}
		}
		if !found {
			return false
		}
	}

	// Check scopes
	if len(subscriber.scopes) > 0 {
		found := false
		for _, scope := range subscriber.scopes {
			if event.Scope == scope {
				found = true
				break
			}
		}
		if !found {
			return false
		}
	}

	// Check additional filters
	for key, value := range subscriber.filters {
		if event.Metadata[key] != value {
			return false
		}
	}

	return true
}

func (s *eventServiceServer) eventMatchesFilters(event *pb.Event, req *pb.SubscribeRequest) bool {
	// Check event types
	if len(req.EventTypes) > 0 {
		found := false
		for _, eventType := range req.EventTypes {
			if event.EventType == eventType {
				found = true
				break
			}
		}
		if !found {
			return false
		}
	}

	// Check scopes
	if len(req.Scopes) > 0 {
		found := false
		for _, scope := range req.Scopes {
			if event.Scope == scope {
				found = true
				break
			}
		}
		if !found {
			return false
		}
	}

	// Check additional filters
	for key, value := range req.Filters {
		if event.Metadata[key] != value {
			return false
		}
	}

	return true
}

func (s *eventServiceServer) getNextSequence(ctx context.Context, scope string) (int64, error) {
	latestSeqInterface, err := s.db.GetLatestSequence(ctx, scope)
	if err != nil && err != sql.ErrNoRows {
		return 0, err
	}

	var latestSeq int64
	if latestSeqInterface != nil {
		if seq, ok := latestSeqInterface.(int64); ok {
			latestSeq = seq
		} else {
			latestSeq = 0
		}
	}

	return latestSeq + 1, nil
}

// Helper function to convert database event to proto event
func dbEventToProto(dbEvent *database.Event) *pb.Event {
	var metadata map[string]string
	if dbEvent.Metadata.Valid && dbEvent.Metadata.String != "" {
		json.Unmarshal([]byte(dbEvent.Metadata.String), &metadata)
	}

	return &pb.Event{
		EventId:   dbEvent.EventID,
		EventType: dbEvent.EventType,
		Scope:     dbEvent.Scope,
		ActorId:   dbEvent.ActorID,
		Timestamp: timestamppb.New(time.Unix(dbEvent.Timestamp, 0)),
		// Note: Payload would need to be reconstructed from JSON if needed
		Metadata: metadata,
		Sequence: dbEvent.Sequence,
	}
}
