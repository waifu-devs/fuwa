# Fuwa Discord App

A Discord-like desktop client built with Go and Raylib that connects to Fuwa gRPC servers.

## Features

- **Discord-like UI**: Clean interface with server sidebar, channel list, and message area
- **Multi-server Support**: Connect to multiple Fuwa servers simultaneously
- **Real-time Messaging**: Live message updates via gRPC streaming
- **Server Management**: Easy add/remove server connections with URL/port input
- **Channel Management**: Create new text channels and navigate between them
- **Channel Navigation**: Browse and switch between text channels
- **Message History**: Scrollable message display with sender information

## Quick Start

1. **Build the app:**
   ```bash
   make build-app
   ```

2. **Run a Fuwa server** (in another terminal):
   ```bash
   make run-server
   ```

3. **Launch the app:**
   ```bash
   ./bin/fuwa-app
   # OR
   make run-app
   ```

4. **Connect to server:**
   - Press `Ctrl+N` to open connection dialog
   - Enter server address (default: `localhost:50051`)
   - Press `Enter` to connect

5. **Create channels:**
   - Press `Ctrl+C` to create a new channel
   - Or click the "+ Create Channel" button
   - Enter channel name and press `Enter`

## Controls

- **Ctrl+N**: Add new server connection
- **Ctrl+C**: Create new channel (when server is selected)
- **Click server icons**: Switch between connected servers  
- **Click channel names**: Switch between channels
- **Click "+ Create Channel"**: Open channel creation dialog
- **Type & Enter**: Send messages
- **Esc**: Cancel dialogs

## Architecture

```
app/
├── main.go              # Raylib UI and event loop
├── types/app.go         # Application state and models
├── client/
│   ├── manager.go       # gRPC connection management
│   └── events.go        # Real-time event handling
└── go.mod              # Dependencies (raylib-go + client)
```

## Dependencies

- **raylib-go**: Cross-platform graphics library
- **gRPC**: Communication with Fuwa servers
- **Client Proto**: Generated gRPC service definitions

## Server Integration

The app integrates with Fuwa's existing gRPC services:

- **ChannelService**: List and create channels
- **MessageService**: Send/receive messages  
- **EventService**: Real-time streaming updates
- **ConfigService**: Server configuration

## UI Layout

- **Left Sidebar (80px)**: Server list with circular icons
- **Channel Panel (200px)**: Channel list for selected server
- **Main Area**: Message display with scrolling
- **Bottom Bar**: Message input field
- **Modal Dialogs**: Server connection prompts