package rual

import (
	"bufio"
	"encoding/json"
	"log/slog"
	"net"
	"strings"
	"testing"
	"time"
)

func TestSlogRegistrationFieldsAndShutdown(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	output := make(chan []map[string]any, 1)
	go func() {
		conn, err := listener.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		conn.SetReadDeadline(time.Now().Add(3 * time.Second))
		scanner := bufio.NewScanner(conn)
		var records []map[string]any
		for scanner.Scan() {
			var record map[string]any
			decoder := json.NewDecoder(strings.NewReader(scanner.Text()))
			decoder.UseNumber()
			decoder.Decode(&record)
			records = append(records, record)
		}
		output <- records
	}()
	handler, err := New(listener.Addr().String(), "worker")
	if err != nil {
		t.Fatal(err)
	}
	logger := slog.New(handler).With("service", "api")
	logger.Error("one\ntwo", slog.Group("request", slog.Int64("id", 9007199254740993)),
		"msg", 42, "level", "custom", "time", 123)
	handler.Close()
	select {
	case records := <-output:
		if len(records) != 2 || records[0]["type"] != "register" || records[1]["message"] != "one\ntwo" {
			t.Fatalf("records: %#v", records)
		}
		fields := records[1]["fields"].(map[string]any)
		if records[1]["level"] != "ERROR" || fields["msg"].(json.Number).String() != "42" || fields["level"] != "custom" {
			t.Fatal(records[1])
		}
		if fields["service"] != "api" || fields["request"].(map[string]any)["id"].(json.Number).String() != "9007199254740993" {
			t.Fatal(fields)
		}
	case <-time.After(4 * time.Second):
		t.Fatal("adapter did not close")
	}
}

func TestUnavailableReceiverDoesNotBlockAndCountsDrops(t *testing.T) {
	listener, _ := net.Listen("tcp", "127.0.0.1:0")
	address := listener.Addr().String()
	listener.Close()
	handler, err := New(address, "worker")
	if err != nil {
		t.Fatal(err)
	}
	start := time.Now()
	logger := slog.New(handler)
	for i := 0; i < 500; i++ {
		logger.Info("offline")
	}
	logger.Info(strings.Repeat("x", 70_000))
	if time.Since(start) > time.Second {
		t.Fatal("logging blocked")
	}
	handler.Close()
	if handler.Dropped() == 0 {
		t.Fatal("missing drop count")
	}
	if _, err := New("0.0.0.0:7331", "unsafe"); err == nil {
		t.Fatal("non-loopback accepted")
	}
}
