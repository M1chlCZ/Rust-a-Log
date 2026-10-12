package io.github.m1chlcz.rual;

import com.google.gson.Gson;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.nio.ByteBuffer;
import java.nio.channels.SocketChannel;
import java.nio.charset.StandardCharsets;
import java.time.Instant;
import java.util.Map;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;

/** A bounded, best-effort sender. Network writes run on a daemon thread. */
public final class RualClient implements AutoCloseable {
    private static final Gson JSON = new Gson();
    private final ArrayBlockingQueue<byte[]> queue = new ArrayBlockingQueue<>(64);
    private final AtomicLong dropped = new AtomicLong();
    private final InetSocketAddress address;
    private final String source;
    private final Thread worker;
    private volatile SocketChannel socket;
    private volatile boolean closed;
    private volatile long closeDeadline;

    public RualClient(String source, String host, int port) throws Exception {
        var ip = InetAddress.getByName(host);
        if (!ip.isLoopbackAddress() || port < 1 || port > 65535) throw new IllegalArgumentException("Use a loopback IP and port");
        if (source.isBlank() || source.getBytes(StandardCharsets.UTF_8).length > 128) throw new IllegalArgumentException("Invalid source");
        this.source = source;
        address = new InetSocketAddress(ip, port);
        worker = new Thread(this::send, "rual-sender");
        worker.setDaemon(true);
        worker.start();
    }

    public void log(String level, String message, Map<String, ?> fields) {
        try {
            var bytes = encode(Map.of("type", "log", "timestamp", Instant.now().toString(),
                "level", level, "message", message, "fields", fields));
            if (closed || bytes.length > 65536 || !queue.offer(bytes)) dropped.incrementAndGet();
        } catch (RuntimeException error) { dropped.incrementAndGet(); }
    }

    public long dropped() { return dropped.get(); }

    private static byte[] encode(Object value) {
        return (JSON.toJson(value) + "\n").getBytes(StandardCharsets.UTF_8);
    }

    private void connect() throws Exception {
        socket = SocketChannel.open();
        socket.configureBlocking(false);
        socket.connect(address);
        long deadline = System.nanoTime() + 100_000_000L;
        while (!socket.finishConnect()) {
            if (System.nanoTime() > deadline) throw new java.io.IOException("connect timeout");
            Thread.sleep(1);
        }
        write(encode(Map.of("type", "register", "version", 1, "source", source, "language", "java")));
    }

    private void write(byte[] bytes) throws Exception {
        var buffer = ByteBuffer.wrap(bytes);
        long deadline = System.nanoTime() + 200_000_000L;
        while (buffer.hasRemaining()) {
            socket.write(buffer);
            if (buffer.hasRemaining()) {
                if (System.nanoTime() > deadline) throw new java.io.IOException("write timeout");
                Thread.sleep(1);
            }
        }
    }

    private void disconnect() {
        if (socket != null) { try { socket.close(); } catch (Exception ignored) { } socket = null; }
    }

    private void send() {
        long reported = 0, nextConnect = 0;
        try {
            while (!closed || (!queue.isEmpty() && System.nanoTime() < closeDeadline)) {
                var bytes = queue.poll(20, TimeUnit.MILLISECONDS);
                if (bytes == null) continue;
                try {
                    if (socket == null) {
                        if (System.nanoTime() < nextConnect) { dropped.incrementAndGet(); continue; }
                        connect();
                    }
                    long lost = dropped.get();
                    if (lost > reported) write(encode(Map.of("type", "dropped", "count", lost - reported)));
                    write(bytes);
                    reported = lost;
                } catch (Exception error) {
                    dropped.incrementAndGet();
                    disconnect();
                    nextConnect = System.nanoTime() + 200_000_000L;
                }
            }
        } catch (InterruptedException error) { Thread.currentThread().interrupt(); }
        finally {
            dropped.addAndGet(queue.size());
            queue.clear();
            disconnect();
        }
    }

    @Override public void close() {
        closeDeadline = System.nanoTime() + 500_000_000L;
        closed = true;
        try {
            worker.join(600);
            if (worker.isAlive()) { worker.interrupt(); worker.join(300); }
        } catch (InterruptedException error) { Thread.currentThread().interrupt(); }
    }
}
