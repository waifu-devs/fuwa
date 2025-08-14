# AGENTS.md - Fuwa Development Guide

## Build/Test Commands
- `make build-server` - Build server binary to bin/fuwa-server
- `make build-client` - Build client binary to bin/fuwa-client  
- `make run-server` - Run gRPC server (auto-generates proto)
- `make run-client` - Run example client (auto-generates proto)
- `make proto-gen` - Generate Go code from proto files
- `go test ./...` - Run tests (from workspace root or module dirs)
- `cd server && sqlc generate` - Regenerate database code after schema changes

## Code Style & Conventions
- **Go Workspace**: Always run commands from workspace root `/home/shixzie/code/waifu-devs/fuwa`
- **Package Structure**: Clean separation - `server/` for core logic, `database/` for data access
- **Imports**: Standard library first, third-party, then local packages  
- **Database**: Use sqlc - never edit generated `.go` files in `database/`, only edit `queries/*.sql` and `migrations/*.sql`
- **Config**: All environment variables use `FUWA_` prefix (e.g., `FUWA_PORT`, `FUWA_DATABASE_URL`)
- **Migrations**: Timestamp format `YYYYMMDDHHMMSS_description.sql` with goose `-- +goose Up/Down`
- **Generated Code**: Never manually edit proto-generated files, always use `make proto-gen`
- **Error Handling**: Include detailed context in error messages for config validation
- **Dependencies**: Install protoc-gen-go tools with `make install-deps`

## Key Files
- `go.work` - Workspace configuration (server + client modules)
- `sqlc.yaml` - Database code generation config (SQLite, JSON tags)
- `Makefile` - Primary build system with proto generation