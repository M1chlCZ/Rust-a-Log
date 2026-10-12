// Package rual forwards slog records to a local rust-a-log receiver.
package rual

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net"
	"sync"
	"sync/atomic"
	"time"
)

const maxFrame = 64 * 1024

type sender struct {
	address, source string
	queue           chan []byte
	done            chan struct{}
	mu              sync.Mutex
	closed          bool
	dropped         atomic.Uint64
	deadline        time.Time
}

// Handler keeps slog attributes and groups through the standard JSON handler.
// Add this handler as an extra output if your application needs durable logs.
type Handler struct {
	slog.Handler
	sender *sender
}

// New starts a background sender. It does not wait for the receiver.
func New(address, source string) (*Handler, error) {
	host, _, err := net.SplitHostPort(address)
	if err != nil || net.ParseIP(host) == nil || !net.ParseIP(host).IsLoopback() {
		return nil, errors.New("rual address must use a loopback IP and port")
	}
	if len(source) == 0 || len(source) > 128 {
		return nil, errors.New("source must contain 1 to 128 bytes")
	}
	sender := &sender{address: address, source: source, queue: make(chan []byte, 64), done: make(chan struct{})}
	handler := &Handler{Handler: slog.NewJSONHandler(sender, &slog.HandlerOptions{Level: slog.LevelDebug}), sender: sender}
	go sender.run()
	return handler, nil
}

func (s *sender) Write(data []byte) (int, error) {
	original := len(data)
	decoder := json.NewDecoder(bytes.NewReader(data))
	if _, err := decoder.Token(); err != nil {
		s.dropped.Add(1)
		return original, nil
	}
	frame := map[string]any{"type": "log"}
	attributes := make(map[string]json.RawMessage)
	afterMessage := false
	// slog emits built-in metadata before msg, then user attributes. Preserve
	// duplicate user names and raw JSON numbers instead of decoding float64.
	for decoder.More() {
		token, err := decoder.Token()
		var value json.RawMessage
		if err != nil || decoder.Decode(&value) != nil {
			s.dropped.Add(1)
			return original, nil
		}
		key := token.(string)
		if !afterMessage && key == "time" {
			frame["timestamp"] = value
		} else if !afterMessage && key == "level" {
			frame["level"] = value
		} else if !afterMessage && key == "msg" {
			frame["message"] = value
			afterMessage = true
		} else {
			attributes[key] = value
		}
	}
	frame["fields"] = attributes
	data, err := json.Marshal(frame)
	if err != nil || len(data)+1 > maxFrame {
		s.dropped.Add(1)
		return original, nil
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed {
		s.dropped.Add(1)
		return original, nil
	}
	select {
	case s.queue <- append(data, '\n'):
	default:
		s.dropped.Add(1)
	}
	// io.Writer must report the original byte count, not the transformed size.
	return original, nil
}

func (s *sender) run() {
	defer close(s.done)
	var conn net.Conn
	var nextDial time.Time
	var reported uint64
	defer func() {
		if conn != nil {
			conn.Close()
		}
	}()
	for {
		s.mu.Lock()
		closed, deadline := s.closed, s.deadline
		s.mu.Unlock()
		if closed && (len(s.queue) == 0 || time.Now().After(deadline)) {
			s.dropped.Add(uint64(len(s.queue)))
			return
		}
		var frame []byte
		select {
		case frame = <-s.queue:
		case <-time.After(20 * time.Millisecond):
			continue
		}
		if conn == nil && time.Now().Before(nextDial) {
			s.dropped.Add(1)
			continue
		}
		if conn == nil {
			var err error
			conn, err = net.DialTimeout("tcp", s.address, 100*time.Millisecond)
			if err != nil {
				nextDial = time.Now().Add(200 * time.Millisecond)
				s.dropped.Add(1)
				continue
			}
			registration, _ := json.Marshal(map[string]any{"type": "register", "version": 1, "source": s.source, "language": "go"})
			conn.SetWriteDeadline(time.Now().Add(100 * time.Millisecond))
			if _, err = conn.Write(append(registration, '\n')); err != nil {
				conn.Close()
				conn = nil
				s.dropped.Add(1)
				continue
			}
		}
		conn.SetWriteDeadline(time.Now().Add(100 * time.Millisecond))
		writer := bufio.NewWriter(conn)
		lost := s.dropped.Load()
		if lost > reported {
			json.NewEncoder(writer).Encode(map[string]any{"type": "dropped", "count": lost - reported})
		}
		_, err := writer.Write(frame)
		if err == nil {
			err = writer.Flush()
		}
		if err != nil {
			conn.Close()
			conn = nil
			s.dropped.Add(1)
		} else {
			reported = lost
		}
	}
}

// Dropped returns the number of records discarded by this handler.
func (h *Handler) Dropped() uint64 { return h.sender.dropped.Load() }

// Close waits up to one second for the sender and its bounded queue.
func (h *Handler) Close() error {
	h.sender.mu.Lock()
	if !h.sender.closed {
		h.sender.closed = true
		h.sender.deadline = time.Now().Add(500 * time.Millisecond)
	}
	h.sender.mu.Unlock()
	<-h.sender.done
	return nil
}

var _ io.Writer = (*sender)(nil)
