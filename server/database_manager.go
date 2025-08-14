package server

import (
	"database/sql"
	"embed"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strings"

	"github.com/pressly/goose/v3"
	"github.com/tursodatabase/go-libsql"
	_ "github.com/tursodatabase/go-libsql"
	"github.com/waifu-devs/fuwa/server/database"
)

//go:embed database/migrations/*.sql
var embedMigrations embed.FS

type MultiDatabaseManager struct {
	connections map[string]*sql.DB
	queries     map[string]*database.Queries
	dataPath    string
	config      *Config
}

func NewMultiDatabaseManager(config *Config) *MultiDatabaseManager {
	return &MultiDatabaseManager{
		connections: make(map[string]*sql.DB),
		queries:     make(map[string]*database.Queries),
		dataPath:    config.DataPath,
		config:      config,
	}
}

func (mdm *MultiDatabaseManager) ReadAllDatabases() error {
	// Ensure data path directory exists
	if err := os.MkdirAll(mdm.dataPath, 0755); err != nil {
		return fmt.Errorf("failed to create data directory %s: %w", mdm.dataPath, err)
	}

	pattern := filepath.Join(mdm.dataPath, "*.db")
	matches, err := filepath.Glob(pattern)
	if err != nil {
		return fmt.Errorf("failed to scan for database files in %s: %w", mdm.dataPath, err)
	}

	if len(matches) == 0 {
		log.Printf("No database files found in %s, creating default database 'fuwa'", mdm.dataPath)
		// Create a default database named 'fuwa' if none exist
		if err := mdm.CreateDatabase("fuwa"); err != nil {
			return fmt.Errorf("failed to create default database: %w", err)
		}
		return nil
	}

	log.Printf("Found %d database files in %s", len(matches), mdm.dataPath)

	for _, dbPath := range matches {
		dbName := strings.TrimSuffix(filepath.Base(dbPath), ".db")

		if err := mdm.openDatabase(dbName, dbPath); err != nil {
			log.Printf("Warning: Failed to open database %s: %v", dbPath, err)
			continue
		}

		log.Printf("Successfully connected to database: %s", dbName)
	}

	if len(mdm.connections) == 0 {
		log.Printf("Warning: No database connections established, server will run without databases")
	}

	return nil
}

func (mdm *MultiDatabaseManager) openDatabase(name, path string) error {
	var db *sql.DB
	var err error

	if mdm.config.TursoURL != "" {
		var options []libsql.Option
		if mdm.config.TursoAuthToken != "" {
			options = append(options, libsql.WithAuthToken(mdm.config.TursoAuthToken))
		}
		if mdm.config.EncryptionKey != "" {
			options = append(options, libsql.WithEncryption(mdm.config.EncryptionKey))
		}

		connector, err := libsql.NewEmbeddedReplicaConnector(path, mdm.config.TursoURL, options...)
		if err != nil {
			return fmt.Errorf("failed to create embedded replica connector for %s: %w", path, err)
		}
		db = sql.OpenDB(connector)
	} else {
		var dsn string
		if mdm.config.EncryptionKey != "" {
			dsn = fmt.Sprintf("file:%s?_encryption_key=%s", path, mdm.config.EncryptionKey)
		} else {
			dsn = "file:" + path
		}

		db, err = sql.Open("libsql", dsn)
		if err != nil {
			return fmt.Errorf("failed to open database %s: %w", path, err)
		}
	}

	if err := db.Ping(); err != nil {
		db.Close()
		return fmt.Errorf("failed to ping database %s: %w", path, err)
	}

	mdm.connections[name] = db
	mdm.queries[name] = database.New(db)

	return nil
}

func (mdm *MultiDatabaseManager) GetDatabase(name string) (*sql.DB, error) {
	db, exists := mdm.connections[name]
	if !exists {
		// Try to create the database if it doesn't exist
		if err := mdm.CreateDatabase(name); err != nil {
			return nil, fmt.Errorf("database %s not found and failed to create: %w", name, err)
		}
		db, exists = mdm.connections[name]
		if !exists {
			return nil, fmt.Errorf("database %s not found even after creation", name)
		}
	}
	return db, nil
}

func (mdm *MultiDatabaseManager) GetQueries(name string) (*database.Queries, error) {
	queries, exists := mdm.queries[name]
	if !exists {
		// Try to create the database if it doesn't exist
		if err := mdm.CreateDatabase(name); err != nil {
			return nil, fmt.Errorf("queries for database %s not found and failed to create: %w", name, err)
		}
		queries, exists = mdm.queries[name]
		if !exists {
			return nil, fmt.Errorf("queries for database %s not found even after creation", name)
		}
	}
	return queries, nil
}

func (mdm *MultiDatabaseManager) GetPrimaryQueries() (*database.Queries, error) {
	if len(mdm.queries) == 0 {
		return nil, nil
	}

	if queries, exists := mdm.queries["fuwa"]; exists {
		return queries, nil
	}

	for _, queries := range mdm.queries {
		return queries, nil
	}

	return nil, nil
}

func (mdm *MultiDatabaseManager) ListDatabases() []string {
	var names []string
	for name := range mdm.connections {
		names = append(names, name)
	}
	return names
}

// CreateDatabase creates a new database file with the given name and runs migrations
func (mdm *MultiDatabaseManager) CreateDatabase(name string) error {
	// Check if database already exists
	if _, exists := mdm.connections[name]; exists {
		return fmt.Errorf("database %s already exists", name)
	}

	// Ensure data path directory exists
	if err := os.MkdirAll(mdm.dataPath, 0755); err != nil {
		return fmt.Errorf("failed to create data directory %s: %w", mdm.dataPath, err)
	}

	dbPath := filepath.Join(mdm.dataPath, name+".db")

	// Create and open the database
	if err := mdm.openDatabase(name, dbPath); err != nil {
		return fmt.Errorf("failed to create database %s: %w", name, err)
	}

	// Run migrations on the newly created database
	if err := mdm.runMigrations(name); err != nil {
		// Clean up the failed database connection
		if db, exists := mdm.connections[name]; exists {
			db.Close()
			delete(mdm.connections, name)
			delete(mdm.queries, name)
		}
		// Remove the database file
		os.Remove(dbPath)
		return fmt.Errorf("failed to run migrations on database %s: %w", name, err)
	}

	log.Printf("Successfully created and initialized database: %s", name)
	return nil
}

// runMigrations applies database migrations using goose
func (mdm *MultiDatabaseManager) runMigrations(name string) error {
	db, exists := mdm.connections[name]
	if !exists {
		return fmt.Errorf("database connection %s not found", name)
	}

	// Set up goose with embedded migrations
	goose.SetBaseFS(embedMigrations)

	// Set dialect for SQLite
	if err := goose.SetDialect("sqlite3"); err != nil {
		return fmt.Errorf("failed to set goose dialect: %w", err)
	}

	// Apply all migrations
	if err := goose.Up(db, "database/migrations"); err != nil {
		return fmt.Errorf("failed to apply migrations: %w", err)
	}

	return nil
}

func (mdm *MultiDatabaseManager) Close() error {
	var errors []string

	for name, db := range mdm.connections {
		if err := db.Close(); err != nil {
			errors = append(errors, fmt.Sprintf("failed to close database %s: %v", name, err))
		}
	}

	mdm.connections = make(map[string]*sql.DB)
	mdm.queries = make(map[string]*database.Queries)

	if len(errors) > 0 {
		return fmt.Errorf("errors closing databases: %s", strings.Join(errors, "; "))
	}

	return nil
}
