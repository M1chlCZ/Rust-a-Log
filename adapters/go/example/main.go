package main

import (
	rual "github.com/M1chlCZ/Rust-a-Log/adapters/go"
	"log/slog"
	"os"
)

func main() {
	address := os.Getenv("RUAL_ADDRESS")
	if address == "" {
		address = "127.0.0.1:7331"
	}
	handler, err := rual.New(address, "go-example")
	if err != nil {
		panic(err)
	}
	defer handler.Close()
	logger := slog.New(handler)
	logger.Info("Application started", "port", 8080)
	logger.Error("Request failed", "request_id", "demo-42")
}
