package database

import (
	"fmt"
	"log"
)

func ExampleUsage(dataPath, encryptionKey string) error {
	dm, err := CreateDatabaseFromConfig(dataPath, encryptionKey, "fuwa_server")
	if err != nil {
		return fmt.Errorf("failed to create database: %w", err)
	}
	defer func() {
		if err := dm.Close(); err != nil {
			log.Printf("Error closing database: %v", err)
		}
	}()

	db := dm.GetDB()

	_, err = db.Exec("CREATE TABLE IF NOT EXISTS test (id INTEGER PRIMARY KEY, name TEXT)")
	if err != nil {
		return fmt.Errorf("failed to create test table: %w", err)
	}

	for i := 0; i < 5; i++ {
		_, err = db.Exec("INSERT INTO test (name) VALUES (?)", fmt.Sprintf("test-%d", i))
		if err != nil {
			return fmt.Errorf("failed to insert test data: %w", err)
		}
	}

	rows, err := db.Query("SELECT id, name FROM test")
	if err != nil {
		return fmt.Errorf("failed to query test data: %w", err)
	}
	defer rows.Close()

	fmt.Printf("Database created at: %s\n", dm.GetDBPath())
	fmt.Println("Test data:")
	for rows.Next() {
		var id int
		var name string
		if err := rows.Scan(&id, &name); err != nil {
			return fmt.Errorf("failed to scan row: %w", err)
		}
		fmt.Printf("  ID: %d, Name: %s\n", id, name)
	}

	return rows.Err()
}

func ExampleTursoUsage(dataPath, tursoURL, authToken, encryptionKey string) error {
	dm, err := CreateDatabaseWithTurso(dataPath, "fuwa_server", tursoURL, authToken, encryptionKey)
	if err != nil {
		return fmt.Errorf("failed to create database with Turso sync: %w", err)
	}
	defer func() {
		if err := dm.Close(); err != nil {
			log.Printf("Error closing database: %v", err)
		}
	}()

	framesSynced, err := dm.Sync()
	if err != nil {
		return fmt.Errorf("failed to sync with Turso: %w", err)
	}
	fmt.Printf("Synced %d frames from Turso\n", framesSynced)

	db := dm.GetDB()

	_, err = db.Exec("CREATE TABLE IF NOT EXISTS test (id INTEGER PRIMARY KEY, name TEXT)")
	if err != nil {
		return fmt.Errorf("failed to create test table: %w", err)
	}

	_, err = db.Exec("INSERT INTO test (name) VALUES (?)", "local-entry")
	if err != nil {
		return fmt.Errorf("failed to insert local test data: %w", err)
	}

	rows, err := db.Query("SELECT id, name FROM test")
	if err != nil {
		return fmt.Errorf("failed to query test data: %w", err)
	}
	defer rows.Close()

	fmt.Printf("Database with Turso sync created at: %s\n", dm.GetDBPath())
	fmt.Printf("Has Turso sync: %v\n", dm.HasTursoSync())
	fmt.Println("Data (local + synced):")
	for rows.Next() {
		var id int
		var name string
		if err := rows.Scan(&id, &name); err != nil {
			return fmt.Errorf("failed to scan row: %w", err)
		}
		fmt.Printf("  ID: %d, Name: %s\n", id, name)
	}

	return rows.Err()
}
