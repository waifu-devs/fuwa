module github.com/waifu-devs/fuwa/app

go 1.24.5

require (
	github.com/gen2brain/raylib-go/raylib v0.55.1
	github.com/waifu-devs/fuwa/client v0.0.0-00010101000000-000000000000
	google.golang.org/grpc v1.74.2
)

require (
	github.com/ebitengine/purego v0.7.1 // indirect
	golang.org/x/exp v0.0.0-20240506185415-9bf2ced13842 // indirect
	golang.org/x/net v0.40.0 // indirect
	golang.org/x/sys v0.33.0 // indirect
	golang.org/x/text v0.25.0 // indirect
	google.golang.org/genproto/googleapis/rpc v0.0.0-20250528174236-200df99c418a // indirect
	google.golang.org/protobuf v1.36.7 // indirect
)

replace github.com/waifu-devs/fuwa/client => ../client
