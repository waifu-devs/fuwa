package server

import (
	"bufio"
	"fmt"
	"os"
	"path"
	"strconv"
	"strings"
)

type Config struct {
	DataPath       string
	Port           int
	Host           string
	DatabaseURL    string
	LogLevel       string
	JWTSecret      string
	AllowedOrigins string
	Environment    string
	TursoURL       string
	TursoAuthToken string
	EncryptionKey  string
}

func LoadConfig() (*Config, error) {
	config := &Config{
		DataPath:       ".",
		Port:           8080,
		Host:           "localhost",
		LogLevel:       "info",
		Environment:    "development",
		AllowedOrigins: "*",
	}

	envVars, err := loadEnvFile(".env")
	if err != nil && !os.IsNotExist(err) {
		return nil, fmt.Errorf("error loading .env file: %w", err)
	}

	config.applyEnvVars(envVars)
	config.applyFuwaEnvVars()

	if err := config.validate(); err != nil {
		return nil, fmt.Errorf("config validation failed: %w", err)
	}

	return config, nil
}

func loadEnvFile(filename string) (map[string]string, error) {
	file, err := os.Open(filename)
	if err != nil {
		return nil, err
	}
	defer file.Close()

	envVars := make(map[string]string)
	scanner := bufio.NewScanner(file)

	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())

		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}

		parts := strings.SplitN(line, "=", 2)
		if len(parts) != 2 {
			continue
		}

		key := strings.TrimSpace(parts[0])
		value := strings.TrimSpace(parts[1])

		if strings.HasPrefix(value, "\"") && strings.HasSuffix(value, "\"") {
			value = strings.Trim(value, "\"")
		} else if strings.HasPrefix(value, "'") && strings.HasSuffix(value, "'") {
			value = strings.Trim(value, "'")
		}

		envVars[key] = value
	}

	return envVars, scanner.Err()
}

func (c *Config) applyEnvVars(envVars map[string]string) {
	if data, exists := envVars["FUWA_DATA_PATH"]; exists {
		c.DataPath = data
	} else {
		data, err := os.UserHomeDir()
		if err != nil {
			c.DataPath = "."
		} else {
			c.DataPath = path.Join(data, ".fuwa")
		}
	}
	if port, exists := envVars["FUWA_PORT"]; exists {
		if p, err := strconv.Atoi(port); err == nil {
			c.Port = p
		}
	}
	if host, exists := envVars["FUWA_HOST"]; exists {
		c.Host = host
	}
	if dbURL, exists := envVars["FUWA_DATABASE_URL"]; exists {
		c.DatabaseURL = dbURL
	}
	if logLevel, exists := envVars["FUWA_LOG_LEVEL"]; exists {
		c.LogLevel = logLevel
	}
	if jwtSecret, exists := envVars["FUWA_JWT_SECRET"]; exists {
		c.JWTSecret = jwtSecret
	}
	if tursoURL, exists := envVars["FUWA_TURSO_URL"]; exists {
		c.TursoURL = tursoURL
	}
	if tursoToken, exists := envVars["FUWA_TURSO_AUTH_TOKEN"]; exists {
		c.TursoAuthToken = tursoToken
	}
	if encKey, exists := envVars["FUWA_ENCRYPTION_KEY"]; exists {
		c.EncryptionKey = encKey
	}
	if env, exists := envVars["FUWA_ENVIRONMENT"]; exists {
		c.Environment = env
	}
}

func (c *Config) applyFuwaEnvVars() {
	envVars := make(map[string]string)

	envKeys := []string{
		"FUWA_DATA_PATH",
		"FUWA_PORT",
		"FUWA_HOST",
		"FUWA_DATABASE_URL",
		"FUWA_LOG_LEVEL",
		"FUWA_JWT_SECRET",
		"FUWA_ALLOWED_ORIGINS",
		"FUWA_ENVIRONMENT",
		"FUWA_TURSO_URL",
		"FUWA_TURSO_AUTH_TOKEN",
		"FUWA_ENCRYPTION_KEY",
	}

	for _, key := range envKeys {
		if value := os.Getenv(key); value != "" {
			envVars[key] = value
		}
	}

	c.applyEnvVars(envVars)
}

func (c *Config) validate() error {
	if c.Port < 1 || c.Port > 65535 {
		return fmt.Errorf("port must be between 1 and 65535, got %d", c.Port)
	}
	if c.Host == "" {
		return fmt.Errorf("host cannot be empty")
	}
	if c.Environment == "production" && c.JWTSecret == "" {
		return fmt.Errorf("JWT_SECRET is required in production environment")
	}
	if c.EncryptionKey == "" {
		return fmt.Errorf("encryption key is required (set FUWA_ENCRYPTION_KEY)")
	}
	return nil
}

func (c *Config) String() string {
	jwtSecret := c.JWTSecret
	if jwtSecret != "" {
		jwtSecret = "***"
	}

	tursoAuthToken := c.TursoAuthToken
	if tursoAuthToken != "" {
		tursoAuthToken = "***"
	}

	encryptionKey := c.EncryptionKey
	if encryptionKey != "" {
		encryptionKey = "***"
	}

	return fmt.Sprintf(`Config:
  Host: %s
  Port: %d
  Environment: %s
  LogLevel: %s
  DatabaseURL: %s
  JWTSecret: %s
  AllowedOrigins: %s
  TursoURL: %s
  TursoAuthToken: %s
  EncryptionKey: %s`,
		c.Host,
		c.Port,
		c.Environment,
		c.LogLevel,
		c.DatabaseURL,
		jwtSecret,
		c.AllowedOrigins,
		c.TursoURL,
		tursoAuthToken,
		encryptionKey,
	)
}
