package database

import (
	"database/sql"
	"fmt"
	"os"
	"path/filepath"

	"github.com/tursodatabase/go-libsql"
	_ "github.com/tursodatabase/go-libsql"
)

type DatabaseManager struct {
	db            *sql.DB
	connector     *libsql.Connector
	dataPath      string
	dbPath        string
	tursoURL      string
	authToken     string
	encryptionKey string
}

func NewDatabaseManager(dataPath, encryptionKey string) *DatabaseManager {
	return &DatabaseManager{
		dataPath:      dataPath,
		encryptionKey: encryptionKey,
	}
}

func NewDatabaseManagerWithTurso(dataPath, tursoURL, authToken, encryptionKey string) *DatabaseManager {
	return &DatabaseManager{
		dataPath:      dataPath,
		tursoURL:      tursoURL,
		authToken:     authToken,
		encryptionKey: encryptionKey,
	}
}

func (dm *DatabaseManager) CreateDatabase(name string) error {
	if err := os.MkdirAll(dm.dataPath, 0755); err != nil {
		return fmt.Errorf("failed to create data directory %s: %w", dm.dataPath, err)
	}

	dm.dbPath = filepath.Join(dm.dataPath, fmt.Sprintf("%s.db", name))

	if dm.tursoURL != "" {
		var options []libsql.Option
		if dm.authToken != "" {
			options = append(options, libsql.WithAuthToken(dm.authToken))
		}
		if dm.encryptionKey != "" {
			options = append(options, libsql.WithEncryption(dm.encryptionKey))
		}

		connector, err := libsql.NewEmbeddedReplicaConnector(dm.dbPath, dm.tursoURL, options...)
		if err != nil {
			return fmt.Errorf("failed to create embedded replica connector: %w", err)
		}
		dm.connector = connector
		dm.db = sql.OpenDB(connector)
	} else {
		var dsn string
		if dm.encryptionKey != "" {
			dsn = fmt.Sprintf("file:%s?_encryption_key=%s", dm.dbPath, dm.encryptionKey)
		} else {
			dsn = "file:" + dm.dbPath
		}

		db, err := sql.Open("libsql", dsn)
		if err != nil {
			return fmt.Errorf("failed to open database at %s: %w", dm.dbPath, err)
		}
		dm.db = db
	}

	if err := dm.db.Ping(); err != nil {
		dm.Close()
		return fmt.Errorf("failed to ping database: %w", err)
	}

	return nil
}

func (dm *DatabaseManager) Sync() (int, error) {
	if dm.connector == nil {
		return 0, fmt.Errorf("sync is only available with Turso embedded replica connections")
	}

	result, err := dm.connector.Sync()
	if err != nil {
		return 0, err
	}

	return result.FramesSynced, nil
}

func (dm *DatabaseManager) GetDB() *sql.DB {
	return dm.db
}

func (dm *DatabaseManager) GetDBPath() string {
	return dm.dbPath
}

func (dm *DatabaseManager) HasTursoSync() bool {
	return dm.connector != nil
}

func (dm *DatabaseManager) Close() error {
	var dbErr, connErr error

	if dm.db != nil {
		dbErr = dm.db.Close()
		dm.db = nil
	}

	if dm.connector != nil {
		connErr = dm.connector.Close()
		dm.connector = nil
	}

	if dbErr != nil {
		return fmt.Errorf("error closing database: %w", dbErr)
	}
	if connErr != nil {
		return fmt.Errorf("error closing connector: %w", connErr)
	}

	return nil
}

func (dm *DatabaseManager) RunMigrations() error {
	if dm.db == nil {
		return fmt.Errorf("database not initialized")
	}

	return nil
}

func CreateDatabaseFromConfig(dataPath, encryptionKey, dbName string) (*DatabaseManager, error) {
	dm := NewDatabaseManager(dataPath, encryptionKey)

	if err := dm.CreateDatabase(dbName); err != nil {
		return nil, fmt.Errorf("failed to create database %s: %w", dbName, err)
	}

	return dm, nil
}

func CreateDatabaseWithTurso(dataPath, dbName, tursoURL, authToken, encryptionKey string) (*DatabaseManager, error) {
	dm := NewDatabaseManagerWithTurso(dataPath, tursoURL, authToken, encryptionKey)

	if err := dm.CreateDatabase(dbName); err != nil {
		return nil, fmt.Errorf("failed to create database %s with Turso sync: %w", dbName, err)
	}

	return dm, nil
}
