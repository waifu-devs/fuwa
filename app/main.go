package main

import (
	"context"
	"fmt"
	"log"
	"strconv"
	"strings"
	"time"

	rl "github.com/gen2brain/raylib-go/raylib"
	pb "github.com/waifu-devs/fuwa/client/proto"

	"github.com/waifu-devs/fuwa/app/client"
	"github.com/waifu-devs/fuwa/app/types"
)

const (
	WINDOW_WIDTH  = 1200
	WINDOW_HEIGHT = 800

	SIDEBAR_WIDTH = 80
	CHANNEL_WIDTH = 200
	HEADER_HEIGHT = 50
)

func main() {
	rl.InitWindow(WINDOW_WIDTH, WINDOW_HEIGHT, "Fuwa Discord Client")
	rl.SetTargetFPS(60)
	defer rl.CloseWindow()

	app := types.NewAppState()
	app.Window.Width = WINDOW_WIDTH
	app.Window.Height = WINDOW_HEIGHT
	app.UI.SidebarWidth = SIDEBAR_WIDTH
	app.UI.ChannelWidth = CHANNEL_WIDTH

	manager := client.NewManager()
	eventHandler := client.NewEventHandler(manager)
	defer manager.Close()
	defer eventHandler.Close()

	for !rl.WindowShouldClose() {
		handleInput(app, manager, eventHandler)
		handleEvents(app, eventHandler)

		rl.BeginDrawing()
		rl.ClearBackground(rl.Color{54, 57, 63, 255}) // Discord dark background

		drawUI(app)

		rl.EndDrawing()
	}
}

func handleInput(app *types.AppState, manager *client.Manager, eventHandler *client.EventHandler) {
	if app.ShowConnectionDialog {
		handleConnectionDialog(app, manager, eventHandler)
		return
	}

	if app.ShowChannelDialog {
		handleChannelDialog(app, manager, eventHandler)
		return
	}

	if rl.IsKeyPressed(rl.KeyN) && rl.IsKeyDown(rl.KeyLeftControl) {
		app.ShowConnectionDialog = true
		app.ConnectionInput = "localhost:50051"
	}

	if rl.IsKeyPressed(rl.KeyC) && rl.IsKeyDown(rl.KeyLeftControl) && app.CurrentServer != nil {
		app.ShowChannelDialog = true
		app.ChannelNameInput = ""
	}

	mousePos := rl.GetMousePosition()

	if rl.IsMouseButtonPressed(rl.MouseButtonLeft) {
		if mousePos.X < SIDEBAR_WIDTH {
			handleSidebarClick(app, mousePos, manager, eventHandler)
		} else if mousePos.X < SIDEBAR_WIDTH+CHANNEL_WIDTH {
			handleChannelClick(app, mousePos, manager)
		}
	}

	if app.CurrentChannel != nil {
		handleMessageInput(app, manager)
	}
}

func handleConnectionDialog(app *types.AppState, manager *client.Manager, eventHandler *client.EventHandler) {
	key := rl.GetCharPressed()
	if key > 0 {
		app.ConnectionInput += string(rune(key))
	}

	if rl.IsKeyPressed(rl.KeyBackspace) && len(app.ConnectionInput) > 0 {
		app.ConnectionInput = app.ConnectionInput[:len(app.ConnectionInput)-1]
	}

	if rl.IsKeyPressed(rl.KeyEnter) {
		connectToServer(app, manager, eventHandler)
	}

	if rl.IsKeyPressed(rl.KeyEscape) {
		app.ShowConnectionDialog = false
		app.ConnectionInput = ""
	}
}

func handleChannelDialog(app *types.AppState, manager *client.Manager, eventHandler *client.EventHandler) {
	key := rl.GetCharPressed()
	if key > 0 {
		app.ChannelNameInput += string(rune(key))
	}

	if rl.IsKeyPressed(rl.KeyBackspace) && len(app.ChannelNameInput) > 0 {
		app.ChannelNameInput = app.ChannelNameInput[:len(app.ChannelNameInput)-1]
	}

	if rl.IsKeyPressed(rl.KeyEnter) {
		createChannel(app, manager, eventHandler)
	}

	if rl.IsKeyPressed(rl.KeyEscape) {
		app.ShowChannelDialog = false
		app.ChannelNameInput = ""
	}
}

func connectToServer(app *types.AppState, manager *client.Manager, eventHandler *client.EventHandler) {
	address := strings.TrimSpace(app.ConnectionInput)
	if address == "" {
		return
	}

	serverID := fmt.Sprintf("server-%d", len(app.Servers)+1)

	err := manager.Connect(serverID, address)
	if err != nil {
		log.Printf("Failed to connect to server: %v", err)
		return
	}

	server := &types.Server{
		ID:        serverID,
		Name:      address,
		Address:   address,
		Connected: true,
	}

	app.Servers = append(app.Servers, server)
	app.CurrentServer = server

	err = eventHandler.Subscribe(serverID)
	if err != nil {
		log.Printf("Failed to subscribe to events: %v", err)
	}

	go loadChannels(app, manager, serverID)

	app.ShowConnectionDialog = false
	app.ConnectionInput = ""
}

func createChannel(app *types.AppState, manager *client.Manager, eventHandler *client.EventHandler) {
	channelName := strings.TrimSpace(app.ChannelNameInput)
	if channelName == "" || app.CurrentServer == nil {
		return
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	channel, err := manager.CreateChannel(ctx, app.CurrentServer.ID, channelName, pb.ChannelType_CHANNEL_TYPE_TEXT)
	if err != nil {
		log.Printf("Failed to create channel: %v", err)
		return
	}

	// Add channel to current server
	app.CurrentServer.Channels = append(app.CurrentServer.Channels, channel)
	app.CurrentChannel = channel

	app.ShowChannelDialog = false
	app.ChannelNameInput = ""

	go loadMessages(app, manager)
}

func loadChannels(app *types.AppState, manager *client.Manager, serverID string) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	channels, err := manager.ListChannels(ctx, serverID)
	if err != nil {
		log.Printf("Failed to load channels: %v", err)
		return
	}

	for _, server := range app.Servers {
		if server.ID == serverID {
			server.Channels = channels
			if len(channels) > 0 && app.CurrentChannel == nil {
				app.CurrentChannel = channels[0]
				go loadMessages(app, manager)
			}
			break
		}
	}
}

func loadMessages(app *types.AppState, manager *client.Manager) {
	if app.CurrentChannel == nil {
		return
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	messages, err := manager.GetMessages(ctx, app.CurrentChannel.ChannelId, 50)
	if err != nil {
		log.Printf("Failed to load messages: %v", err)
		return
	}

	app.Messages = messages
}

func handleSidebarClick(app *types.AppState, mousePos rl.Vector2, manager *client.Manager, eventHandler *client.EventHandler) {
	serverIndex := int(mousePos.Y-HEADER_HEIGHT) / 60
	if serverIndex >= 0 && serverIndex < len(app.Servers) {
		app.CurrentServer = app.Servers[serverIndex]
		if len(app.CurrentServer.Channels) > 0 {
			app.CurrentChannel = app.CurrentServer.Channels[0]
			go loadMessages(app, manager)
		}
	}
}

func handleChannelClick(app *types.AppState, mousePos rl.Vector2, manager *client.Manager) {
	if app.CurrentServer == nil {
		return
	}

	// Check if clicked on "Create Channel" button
	createButtonY := HEADER_HEIGHT + 35
	if mousePos.Y >= float32(createButtonY) && mousePos.Y <= float32(createButtonY+20) {
		app.ShowChannelDialog = true
		app.ChannelNameInput = ""
		return
	}

	// Check if clicked on a channel (offset by the create button)
	adjustedY := int(mousePos.Y - HEADER_HEIGHT - 65) // 65 = 40 original + 25 for button
	channelIndex := adjustedY / 30
	if channelIndex >= 0 && channelIndex < len(app.CurrentServer.Channels) {
		app.CurrentChannel = app.CurrentServer.Channels[channelIndex]
		go loadMessages(app, manager)
	}
}

func handleMessageInput(app *types.AppState, manager *client.Manager) {
	key := rl.GetCharPressed()
	if key > 0 && key != 13 { // Not Enter
		app.MessageInput += string(rune(key))
	}

	if rl.IsKeyPressed(rl.KeyBackspace) && len(app.MessageInput) > 0 {
		app.MessageInput = app.MessageInput[:len(app.MessageInput)-1]
	}

	if rl.IsKeyPressed(rl.KeyEnter) && len(strings.TrimSpace(app.MessageInput)) > 0 {
		sendMessage(app, manager)
	}
}

func sendMessage(app *types.AppState, manager *client.Manager) {
	content := strings.TrimSpace(app.MessageInput)
	if content == "" || app.CurrentChannel == nil {
		return
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	message, err := manager.SendMessage(ctx, app.CurrentChannel.ChannelId, content)
	if err != nil {
		log.Printf("Failed to send message: %v", err)
		return
	}

	app.Messages = append(app.Messages, message)
	app.MessageInput = ""
}

func handleEvents(app *types.AppState, eventHandler *client.EventHandler) {
	select {
	case message := <-eventHandler.MessageChan:
		if app.CurrentChannel != nil && message.ChannelId == app.CurrentChannel.ChannelId {
			app.Messages = append(app.Messages, message)
		}
	case channel := <-eventHandler.ChannelChan:
		for _, server := range app.Servers {
			if server.ID == channel.ServerId {
				server.Channels = append(server.Channels, channel)
				break
			}
		}
	default:
	}
}

func drawUI(app *types.AppState) {
	drawHeader(app)
	drawSidebar(app)
	drawChannelList(app)
	drawMessageArea(app)
	drawMessageInput(app)

	if app.ShowConnectionDialog {
		drawConnectionDialog(app)
	}

	if app.ShowChannelDialog {
		drawChannelDialog(app)
	}
}

func drawHeader(app *types.AppState) {
	rl.DrawRectangle(0, 0, WINDOW_WIDTH, HEADER_HEIGHT, rl.Color{32, 34, 37, 255})

	title := "Fuwa Discord Client"
	if app.CurrentServer != nil {
		title += " - " + app.CurrentServer.Name
		if app.CurrentChannel != nil {
			title += " #" + app.CurrentChannel.Name
		}
	}

	rl.DrawText(title, 10, 15, 20, rl.Color{220, 221, 222, 255})
	rl.DrawText("Ctrl+N: Add Server", WINDOW_WIDTH-280, 15, 16, rl.Color{114, 118, 125, 255})
	rl.DrawText("Ctrl+C: Create Channel", WINDOW_WIDTH-150, 15, 16, rl.Color{114, 118, 125, 255})
}

func drawSidebar(app *types.AppState) {
	rl.DrawRectangle(0, HEADER_HEIGHT, SIDEBAR_WIDTH, WINDOW_HEIGHT-HEADER_HEIGHT, rl.Color{32, 34, 37, 255})

	y := HEADER_HEIGHT + 10
	for i, server := range app.Servers {
		color := rl.Color{88, 101, 242, 255}
		if server == app.CurrentServer {
			color = rl.Color{114, 137, 218, 255}
		}

		rl.DrawCircle(SIDEBAR_WIDTH/2, int32(y+25), 20, color)
		rl.DrawText(strconv.Itoa(i+1), SIDEBAR_WIDTH/2-5, int32(y+20), 16, rl.White)

		y += 60
	}
}

func drawChannelList(app *types.AppState) {
	x := int32(SIDEBAR_WIDTH)
	rl.DrawRectangle(x, HEADER_HEIGHT, CHANNEL_WIDTH, WINDOW_HEIGHT-HEADER_HEIGHT, rl.Color{47, 49, 54, 255})

	if app.CurrentServer == nil {
		rl.DrawText("No server selected", x+10, HEADER_HEIGHT+20, 16, rl.Color{114, 118, 125, 255})
		return
	}

	rl.DrawText(app.CurrentServer.Name, x+10, HEADER_HEIGHT+10, 18, rl.Color{220, 221, 222, 255})

	// Add "Create Channel" button
	createButtonY := int32(HEADER_HEIGHT + 35)
	createButtonHeight := int32(20)
	rl.DrawRectangleLines(x+5, createButtonY, CHANNEL_WIDTH-10, createButtonHeight, rl.Color{114, 118, 125, 255})
	rl.DrawText("+ Create Channel", x+10, createButtonY+3, 14, rl.Color{114, 118, 125, 255})

	y := int32(HEADER_HEIGHT + 65)
	for _, channel := range app.CurrentServer.Channels {
		color := rl.Color{142, 146, 151, 255}
		if channel == app.CurrentChannel {
			rl.DrawRectangle(x, y-2, CHANNEL_WIDTH, 24, rl.Color{64, 68, 75, 255})
			color = rl.White
		}

		rl.DrawText("# "+channel.Name, x+15, y, 16, color)
		y += 30
	}
}

func drawMessageArea(app *types.AppState) {
	x := int32(SIDEBAR_WIDTH + CHANNEL_WIDTH)
	width := int32(WINDOW_WIDTH) - x
	height := int32(WINDOW_HEIGHT - HEADER_HEIGHT - 60) // Leave space for input

	rl.DrawRectangle(x, HEADER_HEIGHT, width, height, rl.Color{54, 57, 63, 255})

	if app.CurrentChannel == nil {
		rl.DrawText("Select a channel to view messages", x+20, HEADER_HEIGHT+50, 18, rl.Color{114, 118, 125, 255})
		return
	}

	y := int32(HEADER_HEIGHT + 10)
	for _, message := range app.Messages {
		author := message.AuthorId
		if len(author) > 10 {
			author = author[:10] + "..."
		}

		rl.DrawText(author+": "+message.Content, x+10, y, 16, rl.Color{220, 221, 222, 255})
		y += 25

		if y > WINDOW_HEIGHT-120 {
			break
		}
	}
}

func drawMessageInput(app *types.AppState) {
	x := int32(SIDEBAR_WIDTH + CHANNEL_WIDTH)
	y := int32(WINDOW_HEIGHT - 60)
	width := int32(WINDOW_WIDTH) - x

	rl.DrawRectangle(x, y, width, 60, rl.Color{64, 68, 75, 255})

	if app.CurrentChannel != nil {
		inputText := app.MessageInput
		if len(inputText) == 0 {
			inputText = "Type a message..."
		}

		rl.DrawText(inputText, x+10, y+20, 16, rl.Color{220, 221, 222, 255})

		if len(app.MessageInput) > 0 {
			cursorX := x + 10 + rl.MeasureText(app.MessageInput, 16)
			rl.DrawText("|", cursorX, y+20, 16, rl.Color{220, 221, 222, 255})
		}
	}
}

func drawConnectionDialog(app *types.AppState) {
	dialogWidth := int32(400)
	dialogHeight := int32(200)
	dialogX := (WINDOW_WIDTH - dialogWidth) / 2
	dialogY := (WINDOW_HEIGHT - dialogHeight) / 2

	rl.DrawRectangle(0, 0, WINDOW_WIDTH, WINDOW_HEIGHT, rl.Color{0, 0, 0, 128})
	rl.DrawRectangle(dialogX, dialogY, dialogWidth, dialogHeight, rl.Color{54, 57, 63, 255})
	rl.DrawRectangleLines(dialogX, dialogY, dialogWidth, dialogHeight, rl.Color{114, 118, 125, 255})

	rl.DrawText("Add Server", dialogX+20, dialogY+20, 20, rl.Color{220, 221, 222, 255})
	rl.DrawText("Server Address:", dialogX+20, dialogY+60, 16, rl.Color{180, 184, 191, 255})

	inputY := dialogY + 90
	rl.DrawRectangle(dialogX+20, inputY, dialogWidth-40, 30, rl.Color{32, 34, 37, 255})
	rl.DrawText(app.ConnectionInput, dialogX+25, inputY+7, 16, rl.Color{220, 221, 222, 255})

	rl.DrawText("Press Enter to connect, Esc to cancel", dialogX+20, dialogY+140, 14, rl.Color{114, 118, 125, 255})
}

func drawChannelDialog(app *types.AppState) {
	dialogWidth := int32(400)
	dialogHeight := int32(200)
	dialogX := (WINDOW_WIDTH - dialogWidth) / 2
	dialogY := (WINDOW_HEIGHT - dialogHeight) / 2

	rl.DrawRectangle(0, 0, WINDOW_WIDTH, WINDOW_HEIGHT, rl.Color{0, 0, 0, 128})
	rl.DrawRectangle(dialogX, dialogY, dialogWidth, dialogHeight, rl.Color{54, 57, 63, 255})
	rl.DrawRectangleLines(dialogX, dialogY, dialogWidth, dialogHeight, rl.Color{114, 118, 125, 255})

	rl.DrawText("Create Channel", dialogX+20, dialogY+20, 20, rl.Color{220, 221, 222, 255})
	rl.DrawText("Channel Name:", dialogX+20, dialogY+60, 16, rl.Color{180, 184, 191, 255})

	inputY := dialogY + 90
	rl.DrawRectangle(dialogX+20, inputY, dialogWidth-40, 30, rl.Color{32, 34, 37, 255})
	rl.DrawText(app.ChannelNameInput, dialogX+25, inputY+7, 16, rl.Color{220, 221, 222, 255})

	rl.DrawText("Press Enter to create, Esc to cancel", dialogX+20, dialogY+140, 14, rl.Color{114, 118, 125, 255})
}
