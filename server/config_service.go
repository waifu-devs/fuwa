package server

import (
	"context"
	"fmt"
	"strings"
	"time"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
	"google.golang.org/protobuf/types/known/timestamppb"

	pb "github.com/waifu-devs/fuwa/server/proto"
)

type configServiceServer struct {
	pb.UnimplementedConfigServiceServer
	config       *Config
	eventService *eventServiceServer
	configStore  ConfigStore
}

type ConfigStore interface {
	GetConfig(scope, key string) (*pb.ConfigValue, error)
	GetConfigs(scope string, keys []string) (map[string]*pb.ConfigValue, error)
	SetConfig(scope, key string, value *pb.ConfigValue, updatedBy string) (*pb.ConfigValue, error)
	DeleteConfig(scope, key string, deletedBy string) (*pb.ConfigValue, error)
	ListConfigKeys(scope, keyPrefix string) ([]*pb.ConfigInfo, error)
}

func NewConfigServiceServer(config *Config, eventService *eventServiceServer, configStore ConfigStore) *configServiceServer {
	return &configServiceServer{
		config:       config,
		eventService: eventService,
		configStore:  configStore,
	}
}

func (s *configServiceServer) GetConfig(ctx context.Context, req *pb.GetConfigRequest) (*pb.GetConfigResponse, error) {
	if req.Scope == "" {
		return nil, status.Error(codes.InvalidArgument, "scope is required")
	}

	configs := make(map[string]*pb.ConfigValue)

	if req.Scope == "server" || strings.HasPrefix(req.Scope, "server:") {
		serverConfigs := s.getServerConfigs()
		for k, v := range serverConfigs {
			if len(req.Keys) == 0 || contains(req.Keys, k) {
				configs[k] = v
			}
		}
	}

	if s.configStore != nil {
		var storeConfigs map[string]*pb.ConfigValue
		var err error

		if len(req.Keys) == 0 {
			storeConfigs, err = s.configStore.GetConfigs(req.Scope, nil)
		} else {
			storeConfigs, err = s.configStore.GetConfigs(req.Scope, req.Keys)
		}

		if err == nil {
			for k, v := range storeConfigs {
				configs[k] = v
			}
		}
	}

	if !req.IncludeSensitive {
		configs = s.filterSensitiveValues(configs)
	}

	return &pb.GetConfigResponse{
		Configs:     configs,
		Scope:       req.Scope,
		LastUpdated: timestamppb.Now(),
	}, nil
}

func (s *configServiceServer) ListConfigs(ctx context.Context, req *pb.ListConfigsRequest) (*pb.ListConfigsResponse, error) {
	if req.Scope == "" {
		return nil, status.Error(codes.InvalidArgument, "scope is required")
	}

	var configInfos []*pb.ConfigInfo

	if req.Scope == "server" || strings.HasPrefix(req.Scope, "server:") {
		serverConfigInfos := s.getServerConfigInfos()
		configInfos = append(configInfos, serverConfigInfos...)
	}

	if s.configStore != nil {
		storeConfigInfos, err := s.configStore.ListConfigKeys(req.Scope, req.KeyPrefix)
		if err == nil {
			configInfos = append(configInfos, storeConfigInfos...)
		}
	}

	return &pb.ListConfigsResponse{
		Configs:    configInfos,
		TotalCount: int32(len(configInfos)),
	}, nil
}

func (s *configServiceServer) SetConfig(ctx context.Context, req *pb.SetConfigRequest) (*pb.SetConfigResponse, error) {
	if req.Scope == "" {
		return nil, status.Error(codes.InvalidArgument, "scope is required")
	}
	if req.Key == "" {
		return nil, status.Error(codes.InvalidArgument, "key is required")
	}
	if req.Value == nil {
		return nil, status.Error(codes.InvalidArgument, "value is required")
	}

	if s.configStore == nil {
		return nil, status.Error(codes.Unimplemented, "config storage not available")
	}

	actorId := s.getActorFromContext(ctx)

	previousValue, err := s.configStore.SetConfig(req.Scope, req.Key, req.Value, actorId)
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to set config: %v", err)
	}

	eventId, err := s.publishConfigUpdatedEvent(req.Scope, req.Key, previousValue, req.Value, actorId, req.Description)
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to publish config event: %v", err)
	}

	return &pb.SetConfigResponse{
		Success:       true,
		PreviousValue: previousValue,
		EventId:       eventId,
	}, nil
}

func (s *configServiceServer) DeleteConfig(ctx context.Context, req *pb.DeleteConfigRequest) (*pb.DeleteConfigResponse, error) {
	if req.Scope == "" {
		return nil, status.Error(codes.InvalidArgument, "scope is required")
	}
	if req.Key == "" {
		return nil, status.Error(codes.InvalidArgument, "key is required")
	}

	if s.configStore == nil {
		return nil, status.Error(codes.Unimplemented, "config storage not available")
	}

	actorId := s.getActorFromContext(ctx)

	deletedValue, err := s.configStore.DeleteConfig(req.Scope, req.Key, actorId)
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to delete config: %v", err)
	}

	eventId, err := s.publishConfigDeletedEvent(req.Scope, req.Key, deletedValue, actorId, req.Reason)
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to publish config event: %v", err)
	}

	return &pb.DeleteConfigResponse{
		Success:      true,
		DeletedValue: deletedValue,
		EventId:      eventId,
	}, nil
}

func (s *configServiceServer) getServerConfigs() map[string]*pb.ConfigValue {
	configs := make(map[string]*pb.ConfigValue)

	configs["host"] = &pb.ConfigValue{
		Value: &pb.ConfigValue_StringValue{StringValue: s.config.Host},
		Type:  pb.ConfigValueType_CONFIG_VALUE_TYPE_STRING,
	}
	configs["port"] = &pb.ConfigValue{
		Value: &pb.ConfigValue_IntValue{IntValue: int64(s.config.Port)},
		Type:  pb.ConfigValueType_CONFIG_VALUE_TYPE_INT,
	}
	configs["environment"] = &pb.ConfigValue{
		Value: &pb.ConfigValue_StringValue{StringValue: s.config.Environment},
		Type:  pb.ConfigValueType_CONFIG_VALUE_TYPE_STRING,
	}
	configs["log_level"] = &pb.ConfigValue{
		Value: &pb.ConfigValue_StringValue{StringValue: s.config.LogLevel},
		Type:  pb.ConfigValueType_CONFIG_VALUE_TYPE_STRING,
	}
	configs["allowed_origins"] = &pb.ConfigValue{
		Value: &pb.ConfigValue_StringValue{StringValue: s.config.AllowedOrigins},
		Type:  pb.ConfigValueType_CONFIG_VALUE_TYPE_STRING,
	}
	configs["jwt_secret"] = &pb.ConfigValue{
		Value:       &pb.ConfigValue_StringValue{StringValue: "***"},
		Type:        pb.ConfigValueType_CONFIG_VALUE_TYPE_STRING,
		IsSensitive: true,
	}

	return configs
}

func (s *configServiceServer) getServerConfigInfos() []*pb.ConfigInfo {
	now := timestamppb.Now()

	return []*pb.ConfigInfo{
		{
			Key:         "host",
			Type:        pb.ConfigValueType_CONFIG_VALUE_TYPE_STRING,
			Description: "Server host address",
			DefaultValue: &pb.ConfigValue{
				Value: &pb.ConfigValue_StringValue{StringValue: "localhost"},
				Type:  pb.ConfigValueType_CONFIG_VALUE_TYPE_STRING,
			},
			CreatedAt: now,
			UpdatedAt: now,
		},
		{
			Key:         "port",
			Type:        pb.ConfigValueType_CONFIG_VALUE_TYPE_INT,
			Description: "Server port number",
			DefaultValue: &pb.ConfigValue{
				Value: &pb.ConfigValue_IntValue{IntValue: 8080},
				Type:  pb.ConfigValueType_CONFIG_VALUE_TYPE_INT,
			},
			CreatedAt: now,
			UpdatedAt: now,
		},
		{
			Key:         "environment",
			Type:        pb.ConfigValueType_CONFIG_VALUE_TYPE_STRING,
			Description: "Runtime environment",
			DefaultValue: &pb.ConfigValue{
				Value: &pb.ConfigValue_StringValue{StringValue: "development"},
				Type:  pb.ConfigValueType_CONFIG_VALUE_TYPE_STRING,
			},
			CreatedAt: now,
			UpdatedAt: now,
		},
		{
			Key:         "log_level",
			Type:        pb.ConfigValueType_CONFIG_VALUE_TYPE_STRING,
			Description: "Logging level",
			DefaultValue: &pb.ConfigValue{
				Value: &pb.ConfigValue_StringValue{StringValue: "info"},
				Type:  pb.ConfigValueType_CONFIG_VALUE_TYPE_STRING,
			},
			CreatedAt: now,
			UpdatedAt: now,
		},
		{
			Key:         "jwt_secret",
			Type:        pb.ConfigValueType_CONFIG_VALUE_TYPE_STRING,
			Description: "JWT signing secret",
			IsSensitive: true,
			CreatedAt:   now,
			UpdatedAt:   now,
		},
	}
}

func (s *configServiceServer) filterSensitiveValues(configs map[string]*pb.ConfigValue) map[string]*pb.ConfigValue {
	filtered := make(map[string]*pb.ConfigValue)
	for k, v := range configs {
		if v.IsSensitive {
			filtered[k] = &pb.ConfigValue{
				Value:       &pb.ConfigValue_StringValue{StringValue: "***"},
				Type:        v.Type,
				IsSensitive: true,
			}
		} else {
			filtered[k] = v
		}
	}
	return filtered
}

func (s *configServiceServer) publishConfigUpdatedEvent(scope, key string, oldValue, newValue *pb.ConfigValue, updatedBy, description string) (string, error) {
	if s.eventService == nil {
		return "", nil
	}

	eventId := fmt.Sprintf("config-updated-%d", time.Now().UnixNano())

	event := &pb.Event{
		EventId:   eventId,
		EventType: "config.updated",
		Scope:     scope,
		ActorId:   updatedBy,
		Timestamp: timestamppb.Now(),
		Metadata: map[string]string{
			"config_key": key,
		},
		Sequence: time.Now().Unix(),
	}

	_, err := s.eventService.Publish(context.Background(), &pb.PublishRequest{
		Event: event,
	})

	return eventId, err
}

func (s *configServiceServer) publishConfigDeletedEvent(scope, key string, deletedValue *pb.ConfigValue, deletedBy, reason string) (string, error) {
	if s.eventService == nil {
		return "", nil
	}

	eventId := fmt.Sprintf("config-deleted-%d", time.Now().UnixNano())

	event := &pb.Event{
		EventId:   eventId,
		EventType: "config.deleted",
		Scope:     scope,
		ActorId:   deletedBy,
		Timestamp: timestamppb.Now(),
		Metadata: map[string]string{
			"config_key": key,
		},
		Sequence: time.Now().Unix(),
	}

	_, err := s.eventService.Publish(context.Background(), &pb.PublishRequest{
		Event: event,
	})

	return eventId, err
}

func (s *configServiceServer) getActorFromContext(ctx context.Context) string {
	return "system"
}

func contains(slice []string, item string) bool {
	for _, s := range slice {
		if s == item {
			return true
		}
	}
	return false
}
