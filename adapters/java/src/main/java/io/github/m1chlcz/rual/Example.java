package io.github.m1chlcz.rual;

import java.util.logging.Logger;

public final class Example {
    public static void main(String[] args) throws Exception {
        var port = Integer.parseInt(System.getenv().getOrDefault("RUAL_PORT", "7331"));
        try (var handler = new RualHandler("java-example", "127.0.0.1", port)) {
            var logger = Logger.getLogger("Example");
            logger.addHandler(handler);
            logger.info("Application started");
            logger.severe("Request failed: demo-42");
            logger.removeHandler(handler);
        }
    }
}
