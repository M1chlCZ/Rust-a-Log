require 'socket'
require 'json'
require 'time'
require 'thread'

# An extra Logger output with a bounded queue and background socket writes.
module Rual
  class LogDevice
    def initialize(source, host: '127.0.0.1', port: 7331, token: ENV.fetch('RUAL_TOKEN', ''))
      raise ArgumentError, 'remote receiver requires a token' unless ['localhost', '127.0.0.1', '::1'].include?(host) || !token.empty?
      raise ArgumentError, 'invalid source, port, or token' if source.empty? || source.bytesize > 128 || !(1..65535).cover?(port) || token.bytesize > 256
      @source, @host, @port, @token = source, host, port, token
      @queue = SizedQueue.new(64)
      @mutex = Mutex.new
      @dropped, @closed, @deadline = 0, false, Float::INFINITY
      @worker = Thread.new { send_frames }
    end

    def dropped
      @mutex.synchronize { @dropped }
    end

    def discard(count = 1)
      @mutex.synchronize { @dropped += count }
    end

    def log(level, message, fields = {})
      frame = JSON.generate(type: 'log', timestamp: Time.now.utc.iso8601(6), level: level,
                            message: message, fields: fields) + "\n"
      @mutex.synchronize do
        if @closed || frame.bytesize > 65536
          @dropped += 1
        else
          begin
            @queue.push(frame, true)
          rescue ThreadError
            @dropped += 1
          end
        end
      end
    rescue StandardError
      discard
    end

    def write(message)
      begin
        entry = JSON.parse(message)
        log(entry.fetch('level', 'INFO'), entry.fetch('message'), entry.fetch('fields', {}))
      rescue JSON::ParserError, KeyError
        level = message[/\b(DEBUG|INFO|WARN|ERROR|FATAL)\b/] || 'INFO'
        log(level, message.chomp)
      end
      message.bytesize
    end

    def close
      @mutex.synchronize do
        unless @closed
          @deadline = clock + 0.5
          @closed = true
        end
      end
      @worker.join(1)
    end

    private

    def clock
      Process.clock_gettime(Process::CLOCK_MONOTONIC)
    end

    def write_frame(client, frame)
      pending = frame.b
      deadline = clock + 0.2
      until pending.empty?
        written = client.write_nonblock(pending, exception: false)
        if written == :wait_writable
          remaining = deadline - clock
          raise IOError, 'write timeout' if remaining <= 0 || !IO.select(nil, [client], nil, remaining)
        else
          pending = pending.byteslice(written..-1)
        end
      end
    end

    def send_frames
      client, reported, retry_at = nil, 0, 0
      loop do
        closed, deadline = @mutex.synchronize { [@closed, @deadline] }
        break if closed && (@queue.empty? || clock >= deadline)
        begin
          frame = @queue.pop(true)
        rescue ThreadError
          sleep 0.02
          next
        end
        begin
          if client.nil?
            if clock < retry_at
              discard
              next
            end
            client = Socket.tcp(@host, @port, connect_timeout: 0.1, resolv_timeout: 0.1)
            write_frame(client, JSON.generate(type: 'register', version: 1, source: @source,
                                              language: 'ruby', token: @token) + "\n")
          end
          lost = dropped
          write_frame(client, JSON.generate(type: 'dropped', count: lost - reported) + "\n") if lost > reported
          write_frame(client, frame)
          reported = lost
        rescue StandardError
          discard
          client&.close
          client = nil
          retry_at = clock + 0.2
        end
      end
    ensure
      client&.close
      discard(@queue.size)
    end
  end
end
