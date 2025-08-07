package server

type Server struct {
	cfg *Config
}

// New server
func New(cfg *Config) *Server {
	return &Server{
		cfg: cfg,
	}
}
