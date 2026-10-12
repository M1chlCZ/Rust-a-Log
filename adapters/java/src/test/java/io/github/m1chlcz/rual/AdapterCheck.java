package io.github.m1chlcz.rual;

import com.google.gson.JsonParser;
import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.net.ServerSocket;
import java.net.InetAddress;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;
import java.util.logging.Logger;
import ch.qos.logback.classic.LoggerContext;

public final class AdapterCheck {
    public static void main(String[] args) throws Exception {
        try (var server = new ServerSocket(0, 1, InetAddress.getLoopbackAddress())) {
            var received = new CompletableFuture<String>();
            var thread = new Thread(() -> {
                try (var connection = server.accept()) {
                    connection.setSoTimeout(3000);
                    var reader = new BufferedReader(new InputStreamReader(connection.getInputStream()));
                    var header = JsonParser.parseString(reader.readLine()).getAsJsonObject();
                    if (!header.get("type").getAsString().equals("register")) throw new AssertionError(header);
                    received.complete(reader.readLine());
                } catch (Exception error) { received.completeExceptionally(error); }
            });
            thread.setDaemon(true);
            thread.start();
            try (var handler = new RualHandler("java-check", "127.0.0.1", server.getLocalPort())) {
                var logger = Logger.getLogger("rual.check");
                logger.setUseParentHandlers(false);
                logger.addHandler(handler);
                logger.severe("one\ntwo");
                logger.removeHandler(handler);
            }
            var record = JsonParser.parseString(received.get(5, TimeUnit.SECONDS)).getAsJsonObject();
            if (!record.get("message").getAsString().equals("one\ntwo")
                || !record.get("level").getAsString().equals("ERROR")) throw new AssertionError(record);
            try (var client = new RualClient("offline", "127.0.0.1", server.getLocalPort() + 1)) {
                var start = System.nanoTime();
                for (int i = 0; i < 500; i++) client.log("INFO", "offline", java.util.Map.of());
                client.log("INFO", "x".repeat(70000), java.util.Map.of());
                if (System.nanoTime() - start > 1_000_000_000L || client.dropped() == 0) throw new AssertionError("Queue blocked");
            }
        }
        try (var server = new ServerSocket(0, 1, InetAddress.getLoopbackAddress())) {
            var received = new CompletableFuture<String>();
            var thread = new Thread(() -> {
                try (var connection = server.accept()) {
                    connection.setSoTimeout(3000);
                    var reader = new BufferedReader(new InputStreamReader(connection.getInputStream()));
                    reader.readLine();
                    received.complete(reader.readLine());
                } catch (Exception error) { received.completeExceptionally(error); }
            });
            thread.setDaemon(true);
            thread.start();
            var context = new LoggerContext();
            var mdc = new ch.qos.logback.classic.util.LogbackMDCAdapter();
            context.setMDCAdapter(mdc);
            mdc.put("request_id", "42");
            var appender = new RualAppender();
            appender.setContext(context);
            appender.setSource("logback-check");
            appender.setPort(server.getLocalPort());
            appender.start();
            var logger = context.getLogger("logback.check");
            logger.addAppender(appender);
            logger.error("Request failed", new IllegalStateException("demo"));
            appender.stop();
            context.stop();
            var record = JsonParser.parseString(received.get(5, TimeUnit.SECONDS)).getAsJsonObject();
            if (!record.get("message").getAsString().equals("Request failed")
                || !record.getAsJsonObject("fields").get("request_id").getAsString().equals("42")
                || !record.getAsJsonObject("fields").get("exception").getAsString().contains("IllegalStateException")) throw new AssertionError(record);
        }
        System.out.println("java checks passed");
    }
}
