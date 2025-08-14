package types

import (
	rl "github.com/gen2brain/raylib-go/raylib"
	pb "github.com/waifu-devs/fuwa/client/proto"
)

type Server struct {
	ID        string
	Name      string
	Address   string
	Connected bool
	Channels  []*pb.Channel
}

type AppState struct {
	Servers        []*Server
	CurrentServer  *Server
	CurrentChannel *pb.Channel
	Messages       []*pb.Message

	ShowConnectionDialog bool
	ConnectionInput      string

	ShowChannelDialog bool
	ChannelNameInput  string

	MessageInput string
	ScrollOffset float32

	Window struct {
		Width  int32
		Height int32
	}

	UI struct {
		SidebarWidth  float32
		ChannelWidth  float32
		MessageScroll float32
		Font          rl.Font
	}
}

func NewAppState() *AppState {
	return &AppState{
		Servers: make([]*Server, 0),
		UI: struct {
			SidebarWidth  float32
			ChannelWidth  float32
			MessageScroll float32
			Font          rl.Font
		}{
			SidebarWidth:  80,
			ChannelWidth:  200,
			MessageScroll: 0,
		},
	}
}
