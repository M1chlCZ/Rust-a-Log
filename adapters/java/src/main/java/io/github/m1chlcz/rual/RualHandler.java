package io.github.m1chlcz.rual;

import java.io.PrintWriter;
import java.io.StringWriter;
import java.util.HashMap;
import java.util.logging.Handler;
import java.util.logging.Level;
import java.util.logging.LogRecord;
import java.util.logging.SimpleFormatter;

/** An extra output for the JDK logger. Keep existing handlers for durable logs. */
public final class RualHandler extends Handler implements AutoCloseable {
    private final RualClient client;

    public RualHandler(String source, String host, int port) throws Exception {
        client = new RualClient(source, host, port);
        setLevel(Level.ALL);
        setFormatter(new SimpleFormatter());
    }

    @Override public void publish(LogRecord record) {
        if (!isLoggable(record)) return;
        var fields = new HashMap<String, Object>();
        fields.put("logger", record.getLoggerName());
        if (record.getThrown() != null) {
            var stack = new StringWriter();
            record.getThrown().printStackTrace(new PrintWriter(stack));
            fields.put("exception", stack.toString());
        }
        int level = record.getLevel().intValue();
        client.log(level >= 1000 ? "ERROR" : level >= 900 ? "WARNING" : level >= 800 ? "INFO" : level >= 500 ? "DEBUG" : "TRACE",
            getFormatter().formatMessage(record), fields);
    }

    public long dropped() { return client.dropped(); }
    @Override public void flush() { }
    @Override public void close() { client.close(); }
}
